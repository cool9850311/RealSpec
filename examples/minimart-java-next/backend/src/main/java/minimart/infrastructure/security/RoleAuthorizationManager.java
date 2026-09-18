package minimart.infrastructure.security;

import java.util.function.Supplier;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import org.springframework.security.authorization.AuthorizationDecision;
import org.springframework.security.authorization.AuthorizationManager;
import org.springframework.security.authorization.AuthorizationResult;
import org.springframework.security.core.Authentication;
import org.springframework.security.oauth2.server.resource.authentication.JwtAuthenticationToken;
import org.springframework.security.web.access.intercept.RequestAuthorizationContext;

/**
 * Grants a request only when the caller's STORED role is the required one.
 *
 * <p>The role is READ, not believed. The token carries a role claim and this class never consults
 * it: the claim states what was true when the token was issued, and a JWT cannot be withdrawn, so a
 * user demoted an hour ago would stay an admin until it expired. The cost is one indexed
 * primary-key read on a request that is about to do more work than that anyway.
 *
 * <p>The outcomes, and where each one is rendered:
 *
 * <ul>
 *   <li>no session (anonymous) → deny → Spring's exception translation sees an anonymous caller →
 *       entry point → 401 UNAUTHENTICATED, hint cleared;
 *   <li>stored role matches → grant → the handler runs;
 *   <li>stored role differs → deny → access-denied handler → 403 FORBIDDEN, hint untouched. Not a
 *       404: the endpoint is published in openapi.yaml, so its existence is not a secret a 404
 *       could keep; the honest answer is that the caller lacks the authority, which is what 403
 *       says;
 *   <li>subject has no row → {@link UnknownSubjectException} → entry point → 401, hint cleared;
 *   <li>the lookup itself fails → the exception propagates, past exception translation (which
 *       handles only security exceptions), to {@link InternalErrorFilter} → 500 INTERNAL_ERROR. The
 *       caller learns nothing about a failure that is ours; the log keeps the cause.
 * </ul>
 */
final class RoleAuthorizationManager implements AuthorizationManager<RequestAuthorizationContext> {

  private final String requiredRole;
  private final RoleLookup lookup;

  RoleAuthorizationManager(String requiredRole, RoleLookup lookup) {
    this.requiredRole = requiredRole;
    this.lookup = lookup;
  }

  @Override
  public AuthorizationResult authorize(
      Supplier<? extends Authentication> authentication, RequestAuthorizationContext context) {
    if (!(authentication.get() instanceof JwtAuthenticationToken session)) {
      // Anonymous. Denying (rather than throwing) lets exception translation tell anonymous from
      // authenticated, which is what turns this into a 401 and not a 403.
      return new AuthorizationDecision(false);
    }
    long userId = JwtTokens.subjectOf(session.getToken());
    String role;
    try {
      role = lookup.roleOf(userId);
    } catch (DomainException e) {
      if (e.code() == ErrorCode.UNAUTHENTICATED) {
        throw new UnknownSubjectException(userId);
      }
      throw e;
    }
    return new AuthorizationDecision(requiredRole.equals(role));
  }
}
