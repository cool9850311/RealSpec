// One scenario's world: the state every step definition reads and writes.
//
// Mirrors examples/minimart-go-nuxt/frontend/tests/e2e/world.ts, which
// format.yml's preamble says this half of the registry is — "unchanged in
// meaning". What paygate adds is reached through `world.stack` (stack.ts):
// ClickHouse, background-work settlement, a projection rebuild, and the two
// counterparties' own logs, none of which minimart's world needed.

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
 * One thing the browser reported as wrong. `console has no errors`
 * (format.yml) covers three distinct sources and this type keeps them
 * distinguishable.
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
    // Subscribed here, before the scenario's first step: `network request …
    // responded` and `console has no errors` describe the WHOLE scenario
    // rather than whatever happened to be in flight when they ran.
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

  networkLog(): readonly NetworkEntry[] {
    return this.network
  }

  browserErrors(): readonly BrowserError[] {
    return this.errors
  }

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
   * Substitutes every `{varName}` from the bag. A token that survives is an
   * error: it means the feature reads a variable no step ever saved.
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
   * Must happen before the first navigation.
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
   * The string an `@key` locator stands for in the active locale. A key the
   * catalogue does not have is an error here rather than a locator that
   * silently matches nothing.
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
 */
export function watchNetwork(page: Page): NetworkEntry[] {
  const log: NetworkEntry[] = []
  page.on('response', (response: Response) => {
    log.push({
      method: response.request().method(),
      path: new URL(response.url()).pathname,
      status: response.status(),
    })
  })
  return log
}

/**
 * Tracks the requests the page has started and not yet settled — see
 * examples/minimart-go-nuxt/frontend/tests/e2e/world.ts for why Playwright's
 * own `networkidle` lifecycle event cannot answer this after a click.
 */
export function watchInflightRequests(page: Page): Set<Request> {
  const inflight = new Set<Request>()
  page.on('request', (request: Request) => {
    inflight.add(request)
  })
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
    // A request the page CANCELLED is not a request that failed — see
    // minimart's world.ts for the full reasoning (net::ERR_ABORTED is what a
    // link-prefetch still in flight becomes the moment the visitor navigates).
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
 */
export function normaliseText(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}
