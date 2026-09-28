// The locator grammar, and nothing else.
//
// spec/bdd/format.yml admits exactly three forms of element reference, and the
// regex that says so is repeated in every pattern that takes one. This file is
// the runtime half of that decision: it resolves those three forms and refuses
// everything else. CSS selectors, XPath and comma-separated fallback chains are
// not "discouraged" here — they are unreachable, because no step can carry one.
//
//     "<role>:<literal name>"   button:Pay           heading:Reports
//     "<role>:@<i18n.key>"      button:@auth.signIn
//     "testid:<name>"           testid:report-day    testid:report-day-{date}
//
// format.yml's own preamble says this half of the registry is minimart's,
// "unchanged in meaning" — this file mirrors
// examples/minimart-go-nuxt/frontend/tests/e2e/locator.ts accordingly, with the
// ROLES list and LOCATOR_PATTERN copied character for character from
// spec/bdd/format.yml's own alternation (the two registries list the same
// roles, but this fragment must equal THIS project's format.yml or
// `realspec validate` and this runner would disagree about what a step is).

import type { Locator, Page } from '@playwright/test'
import type { World } from './world'
import { normaliseText } from './world'

/** The closed vocabulary, copied from the `<role>` alternation in format.yml. */
export const ROLES = [
  'button', 'link', 'heading', 'textbox', 'checkbox', 'radio', 'combobox', 'option',
  'listitem', 'row', 'cell', 'columnheader', 'table', 'list', 'img', 'dialog', 'alert',
  'status', 'navigation', 'banner', 'main', 'region', 'form', 'tab', 'tabpanel',
  'menuitem', 'progressbar', 'paragraph', 'article', 'group', 'search', 'switch',
  'slider', 'spinbutton', 'separator', 'tooltip', 'treeitem',
] as const

export type Role = (typeof ROLES)[number]

const TESTID_PREFIX = 'testid:'

/**
 * The locator fragment of spec/bdd/format.yml, character for character. Every
 * pattern that takes a locator is built from this one string so that a step
 * definition's regex and the registry's can never drift apart independently.
 */
export const LOCATOR_PATTERN = String.raw`((?:button|link|heading|textbox|checkbox|radio|combobox|option|listitem|row|cell|columnheader|table|list|img|dialog|alert|status|navigation|banner|main|region|form|tab|tabpanel|menuitem|progressbar|paragraph|article|group|search|switch|slider|spinbutton|separator|tooltip|treeitem):(?:@[a-z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9_]*)+|[^"]+)|testid:[a-z][a-z0-9-]*(?:\{[a-z][a-zA-Z0-9]*\}[a-z0-9-]*)?)`

function isRole(value: string): value is Role {
  return (ROLES as readonly string[]).includes(value)
}

/** A locator token, resolved against a scope, with the words to blame it by. */
export interface ResolvedLocator {
  readonly token: string
  /**
   * The token plus what it resolved to — for an `@key` token, the string the
   * key means in the active locale, which is the piece a reader of the failure
   * cannot look up for themselves.
   */
  readonly description: string
  readonly locator: Locator
}

/**
 * Turns one locator token into a Playwright locator, scoped to `scope`.
 * `{contextVar}` segments are substituted first, and only inside a `testid:`
 * token — the only form the grammar lets carry one.
 */
export function resolveLocator(scope: Page | Locator, token: string, world: World): ResolvedLocator {
  if (token.startsWith(TESTID_PREFIX)) {
    const name = world.resolveVars(token.slice(TESTID_PREFIX.length))
    if (name === '') throw new Error(`locator "${token}" names no test id`)
    return {
      token,
      description: name === token.slice(TESTID_PREFIX.length) ? `"${token}"` : `"${token}" (${name})`,
      locator: scope.locator(testIdFamilySelector(name)),
    }
  }

  const separator = token.indexOf(':')
  if (separator === -1) {
    throw new Error(
      `locator "${token}" is not one of the three legal forms ` +
        '("<role>:<name>", "<role>:@<i18n.key>", "testid:<name>")',
    )
  }
  const role = token.slice(0, separator)
  const rawName = token.slice(separator + 1)
  if (!isRole(role)) {
    throw new Error(
      `locator "${token}" starts with "${role}:", which is not an ARIA role this registry admits. ` +
        `Legal roles: ${ROLES.join(', ')}. For an element no role and name can identify, use "testid:<name>".`,
    )
  }
  if (rawName === '') throw new Error(`locator "${token}" names no accessible name`)

  const name = rawName.startsWith('@') ? world.translate(rawName.slice(1)) : rawName
  return {
    token,
    description: rawName.startsWith('@')
      ? `"${token}" (${world.activeLocale} → "${name}")`
      : `"${token}"`,
    // exact: true is the whole point. Playwright's default is a
    // case-insensitive SUBSTRING match on the accessible name, which would let
    // "Pay" match a button reading "Pay again".
    locator: scope.getByRole(role, { name, exact: true }),
  }
}

/**
 * The hyphen-family rule (format.yml, "the locator grammar"): a token matches
 * an element whose `data-testid` is EXACTLY the token, or begins with the
 * token followed by a hyphen — so `report-day-2026-09-18` is counted by
 * `list "testid:report-day" has 2 rows` and addressed by
 * `region "testid:report-day-2026-09-18"`.
 *
 * It is a prefix rule on a hyphen boundary, not a substring match:
 * `report-day` matches neither `report-days-total` nor `unreport-day-1`.
 */
export function testIdFamilySelector(name: string): string {
  const quoted = JSON.stringify(name)
  const quotedFamily = JSON.stringify(`${name}-`)
  return `[data-testid=${quoted}], [data-testid^=${quotedFamily}]`
}

/** The matches of an ambiguous locator, one per line, as a reader would tell them apart. */
export async function describeMatches(locator: Locator, limit = 10): Promise<string> {
  const count = await locator.count()
  const lines: string[] = []
  for (let index = 0; index < Math.min(count, limit); index++) {
    const element = locator.nth(index)
    const [testId, text] = await Promise.all([
      element.getAttribute('data-testid'),
      element.innerText().catch(() => ''),
    ])
    const label = testId ? `data-testid="${testId}"` : 'no data-testid'
    lines.push(`  [${index + 1}] ${label}, text "${truncate(normaliseText(text))}"`)
  }
  if (count > limit) lines.push(`  … and ${count - limit} more`)
  return lines.join('\n')
}

function truncate(text: string, max = 80): string {
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`
}
