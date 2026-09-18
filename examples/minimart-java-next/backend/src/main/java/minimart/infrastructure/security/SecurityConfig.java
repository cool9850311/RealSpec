package minimart.infrastructure.security;

import java.util.List;
import minimart.domain.User;
import minimart.infrastructure.config.AppConfig;
import org.springframework.boot.web.servlet.FilterRegistrationBean;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.core.Ordered;
import org.springframework.security.config.annotation.web.builders.HttpSecurity;
import org.springframework.security.config.annotation.web.configuration.EnableWebSecurity;
import org.springframework.security.config.annotation.web.configurers.AbstractHttpConfigurer;
import org.springframework.security.config.http.SessionCreationPolicy;
import org.springframework.security.oauth2.jwt.JwtDecoder;
import org.springframework.security.oauth2.server.resource.authentication.JwtAuthenticationConverter;
import org.springframework.security.web.SecurityFilterChain;
import org.springframework.security.web.context.SecurityContextHolderFilter;
import org.springframework.security.web.servlet.util.matcher.PathPatternRequestMatcher;
import org.springframework.security.web.util.matcher.OrRequestMatcher;
import org.springframework.security.web.util.matcher.RequestMatcher;

/**
 * The security filter chain: who may call what, and what the answer is when they may not.
 *
 * <p>Every rule is here and nowhere else, so the list below can be read against openapi.yaml's
 * {@code security} entries line by line:
 *
 * <pre>
 *   POST /api/v1/auth/login     public
 *   GET  /api/v1/products       public
 *   /api/v1/me, /api/v1/orders  a valid token cookie
 *   /api/v1/admin/**            a valid token cookie AND users.role = 'admin', read per request
 *   anything else               public — Spring MVC answers 404 for paths the API does not have
 * </pre>
 *
 * The last line is a decision, not an omission. The contract's error codes are a closed set
 * describing this API's failures, and "that path is not part of this API" is not one of them;
 * answering an unknown path with 401 would be inventing contract (and would differ from the Go
 * service, whose authentication middleware is mounted only on the protected routes). The protected
 * rules match whole paths, not method and path, so an undeclared method on a protected path asks
 * for a session before it is told there is no such route.
 *
 * <p>Everything Spring Security would otherwise add for a browser-session application is switched
 * off: there is no HttpSession (so no JSESSIONID), no CSRF token (SameSite=Lax is the CSRF defence
 * of this example, stated in openapi.yaml), no login form, no HTTP Basic, no request cache, and no
 * {@code /logout} — the contract has no logout endpoint, deliberately, and a framework-provided one
 * would be an undocumented route.
 */
@Configuration(proxyBeanMethods = false)
@EnableWebSecurity
public class SecurityConfig {

  /** The routes that require a session, and the only ones on which the token cookie is read. */
  static final RequestMatcher PROTECTED_ROUTES;

  /** The routes that additionally require the stored role to be admin. */
  static final RequestMatcher ADMIN_ROUTES;

  static {
    PathPatternRequestMatcher.Builder paths = PathPatternRequestMatcher.withDefaults();
    ADMIN_ROUTES = paths.matcher("/api/v1/admin/**");
    PROTECTED_ROUTES =
        new OrRequestMatcher(
            List.of(paths.matcher("/api/v1/me"), paths.matcher("/api/v1/orders"), ADMIN_ROUTES));
  }

  /** The token issuer/verifier over JWT_SECRET. */
  @Bean
  public JwtTokens jwtTokens(AppConfig config) {
    return new JwtTokens(config.jwtSecret());
  }

  /**
   * The verifier, as a bean of its own. Exposing it is also what makes Spring Boot's
   * resource-server and in-memory-user auto-configurations back off, so there is exactly one way a
   * token is checked and no generated password is ever printed.
   */
  @Bean
  public JwtDecoder jwtDecoder(JwtTokens tokens) {
    return tokens.decoder();
  }

  /** The cookie policy, with COOKIE_SECURE applied. */
  @Bean
  public SessionCookies sessionCookies(AppConfig config) {
    return new SessionCookies(config.cookieSecure());
  }

  /** The chain itself. See the class comment for the rules. */
  @Bean
  public SecurityFilterChain apiSecurity(
      HttpSecurity http, JwtDecoder decoder, SessionCookies cookies, RoleLookup roleLookup) {
    JsonAuthenticationEntryPoint unauthenticated = new JsonAuthenticationEntryPoint(cookies);
    JsonAccessDeniedHandler forbidden = new JsonAccessDeniedHandler();

    // The token's claims confer no authorities. In particular the role claim is not mapped to
    // anything: authorisation reads users.role (RoleAuthorizationManager), never the token.
    JwtAuthenticationConverter noAuthorities = new JwtAuthenticationConverter();
    noAuthorities.setJwtGrantedAuthoritiesConverter(jwt -> List.of());

    http.csrf(AbstractHttpConfigurer::disable)
        .formLogin(AbstractHttpConfigurer::disable)
        .httpBasic(AbstractHttpConfigurer::disable)
        .logout(AbstractHttpConfigurer::disable)
        .requestCache(AbstractHttpConfigurer::disable)
        .sessionManagement(s -> s.sessionCreationPolicy(SessionCreationPolicy.STATELESS))
        // Authentication, then authorisation, in that order — both in the chain, before any
        // handler runs.
        .oauth2ResourceServer(
            rs ->
                rs.bearerTokenResolver(new CookieTokenResolver(PROTECTED_ROUTES))
                    .authenticationEntryPoint(unauthenticated)
                    .accessDeniedHandler(forbidden)
                    .jwt(jwt -> jwt.decoder(decoder).jwtAuthenticationConverter(noAuthorities)))
        .authorizeHttpRequests(
            routes ->
                routes
                    .requestMatchers(ADMIN_ROUTES)
                    .access(new RoleAuthorizationManager(User.ROLE_ADMIN, roleLookup))
                    .requestMatchers(PROTECTED_ROUTES)
                    .authenticated()
                    .anyRequest()
                    .permitAll())
        .exceptionHandling(
            e -> e.authenticationEntryPoint(unauthenticated).accessDeniedHandler(forbidden))
        // First in the chain, so it wraps authorisation: a failing role lookup is a 500 in the
        // contract's shape rather than the container's error page.
        .addFilterBefore(new InternalErrorFilter(), SecurityContextHolderFilter.class);
    return http.build();
  }

  /**
   * CORS as a servlet filter ahead of everything else, Spring Security included: a preflight is
   * answered before any rule about sessions could refuse it, exactly as the Go middleware answers
   * it before any route is matched.
   */
  @Bean
  public FilterRegistrationBean<CorsFilter> corsFilter(AppConfig config) {
    FilterRegistrationBean<CorsFilter> registration =
        new FilterRegistrationBean<>(new CorsFilter(config.frontendOrigin()));
    registration.setOrder(Ordered.HIGHEST_PRECEDENCE);
    registration.addUrlPatterns("/*");
    return registration;
  }
}
