package minimart.application.usecase;

import java.util.Optional;
import minimart.application.dto.MeResponse;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import minimart.domain.User;
import minimart.infrastructure.repository.UserRepository;

/** Verifies credentials and reads the caller's own profile. */
public class AuthUsecase {

  /**
   * A valid bcrypt hash of a value nobody knows. It is compared against when the username does not
   * exist, so that "no such user" costs the same time as "wrong password" and the endpoint does not
   * leak which usernames are registered through its latency after refusing to leak it through its
   * status code.
   */
  static final String DUMMY_HASH = "$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2";

  /** The stand-in whose password check is run, and whose result is ignored, for unknown names. */
  private static final User NOBODY = new User(0, "", DUMMY_HASH, User.ROLE_GUEST, 0);

  private final UserRepository users;

  /** A use case over {@code users}. */
  public AuthUsecase(UserRepository users) {
    this.users = users;
  }

  /**
   * Returns the user whose password matches.
   *
   * @throws DomainException INVALID_CREDENTIALS for an unknown username and for a wrong password
   *     alike, deliberately
   */
  public User authenticate(String username, String password) {
    Optional<User> found = users.findByUsername(username);
    if (found.isEmpty()) {
      // Spend the bcrypt cost anyway; see DUMMY_HASH. The result is meaningless and discarded.
      NOBODY.verifyPassword(password);
      throw new DomainException(ErrorCode.INVALID_CREDENTIALS);
    }
    User user = found.get();
    if (!user.verifyPassword(password)) {
      throw new DomainException(ErrorCode.INVALID_CREDENTIALS);
    }
    return user;
  }

  /**
   * Returns the role stored on the caller's users row.
   *
   * <p>It exists so that authorisation is answered from the database rather than from the token's
   * role claim. The claim states what was true when the token was issued; a JWT cannot be
   * withdrawn, so a user demoted an hour ago still carries one saying otherwise for the rest of the
   * day.
   *
   * @throws DomainException UNAUTHENTICATED when the subject has no row, for the same reason {@link
   *     #me} gives: the session is what has become invalid
   */
  public String roleOf(long userId) {
    return users
        .findRoleById(userId)
        .orElseThrow(() -> new DomainException(ErrorCode.UNAUTHENTICATED));
  }

  /**
   * Returns the profile of the authenticated caller.
   *
   * @throws DomainException UNAUTHENTICATED, not 404, for a subject that no longer exists: the
   *     session is the thing that is invalid, and the caller's remedy is to log in again rather
   *     than to look for a different user
   */
  public MeResponse me(long userId) {
    User user =
        users.findById(userId).orElseThrow(() -> new DomainException(ErrorCode.UNAUTHENTICATED));
    return new MeResponse(user.username(), user.role(), user.points());
  }
}
