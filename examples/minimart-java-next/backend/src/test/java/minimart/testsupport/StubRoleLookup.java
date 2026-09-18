package minimart.testsupport;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.function.LongFunction;
import minimart.domain.DomainException;
import minimart.domain.ErrorCode;
import minimart.infrastructure.security.RoleLookup;

/**
 * A {@link RoleLookup} whose answer the test decides, and which records whom it was asked about.
 */
public final class StubRoleLookup implements RoleLookup {

  private volatile LongFunction<String> answer = id -> "admin";
  private final List<Long> lookedUp = new CopyOnWriteArrayList<>();

  /** Back to answering "admin" with an empty history. */
  public void reset() {
    answer = id -> "admin";
    lookedUp.clear();
  }

  /** Every lookup answers {@code role}. */
  public void answers(String role) {
    answer = id -> role;
  }

  /** Every lookup finds no users row. */
  public void findsNoUser() {
    answer =
        id -> {
          throw new DomainException(ErrorCode.UNAUTHENTICATED);
        };
  }

  /** Every lookup fails the way a database read fails. */
  public void fails(RuntimeException failure) {
    answer =
        id -> {
          throw failure;
        };
  }

  /** The user ids asked about, in order. */
  public List<Long> lookedUp() {
    return List.copyOf(lookedUp);
  }

  @Override
  public String roleOf(long userId) {
    lookedUp.add(userId);
    return answer.apply(userId);
  }
}
