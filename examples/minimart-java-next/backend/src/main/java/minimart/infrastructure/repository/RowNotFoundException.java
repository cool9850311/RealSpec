package minimart.infrastructure.repository;

import java.io.Serial;

/**
 * A row the operation depends on does not exist. Raised where an {@link java.util.Optional} cannot
 * carry the answer — inside a transaction callback, where only an exception can both report the
 * outcome and roll the transaction back.
 */
public final class RowNotFoundException extends RuntimeException {

  @Serial private static final long serialVersionUID = 1L;

  /** An exception naming what was looked for. */
  public RowNotFoundException(String what) {
    super("repository: no such row: " + what, null, false, false);
  }
}
