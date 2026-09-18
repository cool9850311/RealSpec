package minimart.infrastructure.security;

import minimart.domain.DomainException;

/**
 * Answers "what role does this user hold right now" from the system of record.
 *
 * <p>It is a functional interface rather than a repository or use-case type so that this package
 * keeps depending on nothing above it: the wiring supplies the implementation ({@code
 * AuthUsecase::roleOf}), and {@link RoleAuthorizationManager} knows only that the answer comes from
 * somewhere authoritative. It is also what a unit test replaces to decide every outcome of the role
 * check without a database.
 */
@FunctionalInterface
public interface RoleLookup {

  /**
   * @return the role stored on the users row whose id is {@code userId}
   * @throws DomainException {@code UNAUTHENTICATED} when the id names no user
   */
  String roleOf(long userId);
}
