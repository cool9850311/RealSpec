/**
 * Adapts HTTP to the use cases: it decodes, it validates the shape of the request, and it renders
 * the result. No business rule lives here, and no authorisation either — that is decided in the
 * security filter chain before a handler is reached.
 */
package minimart.infrastructure.web;
