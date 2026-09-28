// The one step that sets a scenario's browser up before it navigates: the
// language it renders in.
//
// There is deliberately no step here that signs a browser in. A scenario that
// needs a dashboard session writes the real form login —
// `visit "/login"`, two `fill` steps and a `click` — which is four lines of
// steps this suite already has (spec/bdd/e2e/login.feature's own header
// comment). See the preamble of spec/bdd/format.yml.

import { Given } from '../fixtures'

/**
 * `locale is "<locale>"`. Must come before the first navigation; see
 * World.setLocale for why that is a failure rather than a warning.
 */
Given(/^locale is "([a-z]{2}_[A-Z]{2})"$/, async ({ world }, locale: string) => {
  await world.setLocale(locale)
})
