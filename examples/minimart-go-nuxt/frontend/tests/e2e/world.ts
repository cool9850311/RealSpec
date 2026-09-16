// One scenario's world: the state every step definition reads and writes.
//
// It is the browser-side counterpart of the `ScenarioCtx` struct in
// backend/src/test/scenario_ctx.go, and it holds the same four things that file
// holds — the context variable bag, the database handle, the record of what
// went over the wire, and the scenario's own addressing — plus the two a browser
// adds: the console log and the active locale's catalogue.
//
// Nothing here knows about Gherkin. The step definitions in steps/ are thin;
// this is where their meaning lives, so that a step's behaviour can be read in
// one place and so that the harness self-tests can exercise it.

import fs from 'node:fs'
import path from 'node:path'
import type { BrowserContext, ConsoleMessage, Page, Request, Response } from '@playwright/test'
import type pg from 'pg'
import { I18N_DIR } from './env'
import type { ScenarioStack } from './stack'

/** The locales the app ships (frontend/i18n). */
export const SUPPORTED_LOCALES = ['en_US', 'zh_TW'] as const
export type AppLocale = (typeof SUPPORTED_LOCALES)[number]

/** frontend/i18n/index.ts — the cookie the app reads its locale from. */
const LOCALE_COOKIE = 'locale'

/** BCP 47 tag sent in Accept-Language for each catalogue locale. */
const ACCEPT_LANGUAGE: Record<AppLocale, string> = {
  en_US: 'en-US,en;q=0.9',
  zh_TW: 'zh-TW,zh-Hant;q=0.9,zh;q=0.8',
}

export function isAppLocale(value: string): value is AppLocale {
  return (SUPPORTED_LOCALES as readonly string[]).includes(value)
}

/** One request the page made, as the assertions need to see it. */
export interface NetworkEntry {
  method: string
  /** Path only: no origin, no query string — what the feature writes. */
  path: string
  status: number
}

/**
 * One thing the browser reported as wrong.
 *
 * `console has no errors` (format.yml) covers three distinct sources and this
 * type keeps them distinguishable, because "an uncaught TypeError" and "a
 * request that never connected" call for different fixes and a failure message
 * that blurred them would send the reader to the wrong place.
 */
export interface BrowserError {
  source: 'console' | 'pageerror' | 'requestfailed'
  text: string
  where: string
}

/** A flat catalogue: dotted key → the string it renders in one locale. */
type Catalogue = Map<string, string>

const catalogues = new Map<AppLocale, Catalogue>()

const CONTEXT_VAR = /\{[a-z][a-zA-Z0-9]*\}/

export class World {
  readonly stack: ScenarioStack
  readonly context: BrowserContext
  readonly page: Page

  /** The bag `{varName}` tokens resolve from, in step text, JSON and SQL. */
  private readonly vars = new Map<string, string>()

  private readonly network: NetworkEntry[]
  private readonly errors: BrowserError[]
  private readonly inflight: Set<Request>

  private locale: AppLocale = 'en_US'
  private navigated = false

  constructor(stack: ScenarioStack, context: BrowserContext, page: Page) {
    this.stack = stack
    this.context = context
    this.page = page
    // Subscribed here, in the constructor, which runs before the scenario's
    // first step: `network request … responded` and `console has no errors`
    // describe the WHOLE scenario rather than whatever happened to be in flight
    // when they ran.
    this.network = watchNetwork(page)
    this.errors = watchBrowserErrors(page)
    this.inflight = watchInflightRequests(page)
  }

  get db(): pg.Client {
    return this.stack.db
  }

  /** The locale `@key` locators are resolved in. */
  get activeLocale(): AppLocale {
    return this.locale
  }

  // ── Recording ─────────────────────────────────────────────────────────────

  /** Every request recorded so far, oldest first. */
  networkLog(): readonly NetworkEntry[] {
    return this.network
  }

  /** Every browser error recorded so far, oldest first. */
  browserErrors(): readonly BrowserError[] {
    return this.errors
  }

  /** How many requests the page has started and not yet finished or failed. */
  inflightRequestCount(): number {
    return this.inflight.size
  }

  markNavigated(): void {
    this.navigated = true
  }

  // ── Context variables ─────────────────────────────────────────────────────

  saveVar(name: string, value: string): void {
    this.vars.set(name, value)
  }

  /**
   * Substitutes every `{varName}` from the bag.
   *
   * A token that survives is an error, exactly as it is on the API side: it
   * means the feature reads a variable no step ever saved, and saying so here
   * beats letting it reach PostgreSQL as a literal or a locator as a string
   * nothing matches.
   */
  resolveVars(text: string): string {
    let resolved = text
    for (const [name, value] of this.vars) {
      resolved = resolved.split(`{${name}}`).join(value)
    }
    const leftover = CONTEXT_VAR.exec(resolved)
    if (leftover) {
      const saved = [...this.vars.keys()].sort()
      throw new Error(
        `unknown context variable ${leftover[0]} (saved so far: ${saved.length > 0 ? saved.join(', ') : 'none'})`,
      )
    }
    return resolved
  }

  // ── Locale ────────────────────────────────────────────────────────────────

  /**
   * Selects the UI locale: the app's cookie, the matching Accept-Language
   * header, and the catalogue this world resolves `@key` locators against.
   *
   * It must happen before the first navigation. A locale applied to an
   * already-rendered page would leave the catalogue and the screen disagreeing,
   * and every `@key` locator silently looking for the wrong string — so that is
   * a failure, not a best effort.
   */
  async setLocale(locale: string): Promise<void> {
    if (!isAppLocale(locale)) {
      throw new Error(`locale "${locale}" is not one the app ships (${SUPPORTED_LOCALES.join(', ')})`)
    }
    if (this.navigated) {
      throw new Error(
        `locale is "${locale}" must appear before the first navigation of the scenario: ` +
          'the page already loaded has been rendered in ' +
          `"${this.locale}", and switching the catalogue now would make every "@key" locator ` +
          'look for a string the screen does not show.',
      )
    }
    this.locale = locale
    await this.context.addCookies([
      { name: LOCALE_COOKIE, value: locale, url: this.stack.baseURL, sameSite: 'Lax' },
    ])
    await this.context.setExtraHTTPHeaders({ 'Accept-Language': ACCEPT_LANGUAGE[locale] })
  }

  /**
   * The string an `@key` locator stands for in the active locale.
   *
   * A key the catalogue does not have is an error here rather than a locator
   * that silently matches nothing: the app renders an unknown key as the raw
   * key, so the resulting failure would be "no element named shop.titl" and the
   * reader would have to work out that the key was the typo.
   */
  translate(key: string): string {
    const catalogue = loadCatalogue(this.locale)
    const value = catalogue.get(key)
    if (value === undefined) {
      throw new Error(
        `i18n key "${key}" is not in the ${this.locale} catalogue (frontend/i18n/${this.locale}.json)`,
      )
    }
    return value
  }
}

/**
 * Records every response the page receives, for as long as the page lives.
 *
 * Returns the growing array rather than taking a callback: the assertion steps
 * want the whole history, and a subscription that hands back its own log is one
 * fewer place for "did we start listening in time?" to be a question.
 */
export function watchNetwork(page: Page): NetworkEntry[] {
  const log: NetworkEntry[] = []
  page.on('response', (response: Response) => {
    log.push({
      method: response.request().method(),
      // Path only: no origin, no query string — what the feature writes.
      path: new URL(response.url()).pathname,
      status: response.status(),
    })
  })
  return log
}

/**
 * Tracks the requests the page has started and not yet settled.
 *
 * Playwright has its own answer — the `networkidle` lifecycle event — and it
 * loses a race that this one cannot.
 *
 * `page.waitForLoadState('networkidle')` resolves on whether `networkidle` is in
 * the frame's fired-lifecycle set AT THE MOMENT OF THE CALL. After a click on an
 * already-loaded page it is: the page was quiet an instant ago, so the event is
 * still there, and the wait returns before the XHR the click just triggered has
 * even been issued. Playwright does remove the event once a request starts —
 * `Frame._recalculateNetworkIdle` deletes it, and three of its four call sites
 * pass no exempt frame, so the deletion does happen — but by then the caller has
 * long since moved on. It is a wait that does not wait, which is worse than no
 * wait, because it reads like one. After `visit` the same call is fine, because
 * the navigation is what the event is about.
 *
 * Counting the requests here fixes it by asking a different question: not "was
 * the page quiet" but "has it been quiet continuously for NETWORK_QUIET_MS". A
 * request that starts fifty milliseconds after the click resets that window, so
 * the race has nowhere to hide. A set, two listeners, no cached flag to go
 * stale. Subscribed in the World constructor, before the scenario's first step,
 * for the same reason the network and console logs are.
 */
export function watchInflightRequests(page: Page): Set<Request> {
  const inflight = new Set<Request>()
  page.on('request', (request: Request) => {
    inflight.add(request)
  })
  // A request leaves exactly one of these two ways, and removing from a Set is
  // idempotent, so a redirect chain or a duplicated event cannot make the count
  // drift below zero the way a counter could.
  page.on('requestfinished', (request: Request) => {
    inflight.delete(request)
  })
  page.on('requestfailed', (request: Request) => {
    inflight.delete(request)
  })
  return inflight
}

/**
 * Records everything the browser reports as wrong, from all three sources
 * `console has no errors` covers.
 *
 * Standalone and exported so the harness self-test can drive it against a bare
 * page and prove all three are caught: an `error`-level console call, an
 * uncaught exception, and a request that never connected. A collector that
 * silently watched only one of them would make the assertion look strict while
 * being blind, which is the exact failure mode this whole standard exists to
 * prevent.
 */
export function watchBrowserErrors(page: Page): BrowserError[] {
  const errors: BrowserError[] = []
  page.on('console', (message: ConsoleMessage) => {
    if (message.type() !== 'error') return
    const at = message.location()
    errors.push({
      source: 'console',
      text: message.text(),
      where: at.url ? `${at.url}:${at.lineNumber}:${at.columnNumber}` : 'unknown location',
    })
  })
  page.on('pageerror', (error: Error) => {
    errors.push({
      source: 'pageerror',
      text: `${error.name}: ${error.message}`,
      where: error.stack?.split('\n')[1]?.trim() ?? 'no stack',
    })
  })
  page.on('requestfailed', (request: Request) => {
    const reason = request.failure()?.errorText ?? 'unknown'
    // A request the page CANCELLED is not a request that failed. Chromium
    // reports both through the same event, and `net::ERR_ABORTED` is what a
    // link-prefetch still in flight becomes the moment the visitor navigates —
    // which happens on every healthy page and is not anybody's bug. The
    // registry's words are "no request failed at the transport level"; a
    // cancellation never reached that level and never produced a wrong answer,
    // and an abort that mattered shows up as a missing element in the
    // assertion that needed it.
    //
    // This is a distinction between two outcomes of one event, not an
    // exemption for a class of message: every genuine failure — DNS, refused
    // connection, TLS, timeout — is still recorded, and so is every
    // error-level console message, whatever it says.
    if (reason === 'net::ERR_ABORTED') return
    errors.push({
      source: 'requestfailed',
      text: `${request.method()} ${request.url()} failed: ${reason}`,
      where: 'transport',
    })
  })
  return errors
}

/** Loads and flattens one locale's catalogue, once per process. */
function loadCatalogue(locale: AppLocale): Catalogue {
  const cached = catalogues.get(locale)
  if (cached) return cached
  const raw: unknown = JSON.parse(fs.readFileSync(path.join(I18N_DIR, `${locale}.json`), 'utf8'))
  const flat: Catalogue = new Map()
  flatten(raw, '', flat)
  catalogues.set(locale, flat)
  return flat
}

function flatten(value: unknown, prefix: string, into: Catalogue): void {
  if (typeof value === 'string') {
    into.set(prefix, value)
    return
  }
  if (typeof value !== 'object' || value === null) return
  for (const [key, child] of Object.entries(value)) {
    flatten(child, prefix === '' ? key : `${prefix}.${key}`, into)
  }
}

/**
 * Whitespace normalisation, applied to every text comparison in the suite:
 * runs of whitespace collapse to one space and the ends are trimmed.
 *
 * PLAN.md §8 R2 left the semantics of reading an element's text open and asked
 * for it to be pinned by tests. This is the pin: it is what `innerText` gives,
 * normalised the way a reader would read it aloud, so that markup indentation
 * never decides whether an assertion passes.
 */
export function normaliseText(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}
