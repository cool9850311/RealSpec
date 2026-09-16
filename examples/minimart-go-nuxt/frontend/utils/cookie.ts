// Reading and deleting the two cookies this app is allowed to see.
//
// `document.cookie` is the only store involved: the session itself is the
// HttpOnly `token` cookie, which script cannot read by design, so nothing here
// can or should try. What is readable is the locale the visitor chose and the
// `session_hint` flag the API sets beside the token — neither carries any
// authority.
//
// Browser-only. Every function returns a harmless answer during prerendering
// rather than throwing, so a caller does not have to repeat the guard.

/** The value of one cookie, or null when it is absent. */
export function readCookie(name: string): string | null {
  if (!import.meta.client) return null
  for (const entry of document.cookie.split(';')) {
    const separator = entry.indexOf('=')
    if (separator === -1) continue
    if (entry.slice(0, separator).trim() !== name) continue
    return decodeURIComponent(entry.slice(separator + 1).trim())
  }
  return null
}

/**
 * Expires one cookie for this origin.
 *
 * Path=/ matches the attribute the API writes; a delete whose path differs
 * creates a second cookie instead of removing the first.
 */
export function deleteCookie(name: string): void {
  if (!import.meta.client) return
  document.cookie = `${name}=; Path=/; Max-Age=0; SameSite=Lax`
}
