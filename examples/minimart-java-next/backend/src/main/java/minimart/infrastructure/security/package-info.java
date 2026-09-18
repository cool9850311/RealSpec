/**
 * The HTTP concerns that are not any one endpoint's: the session (a JWT in a cookie), who may call
 * what, the CORS policy, and the error bodies the filter chain writes before any controller runs.
 *
 * <p>The request path through this package, in order:
 *
 * <pre>
 *   CorsFilter                  servlet filter, before Spring Security; answers every OPTIONS
 *   InternalErrorFilter         first in the security chain; a non-security exception → 500
 *   BearerTokenAuthenticationFilter
 *                               CookieTokenResolver reads the token cookie on protected routes;
 *                               JwtTokens' decoder verifies it; a bad one → JsonAuthenticationEntryPoint
 *   AuthorizationFilter         route rules (SecurityConfig); admin routes → RoleAuthorizationManager
 * </pre>
 */
package minimart.infrastructure.security;
