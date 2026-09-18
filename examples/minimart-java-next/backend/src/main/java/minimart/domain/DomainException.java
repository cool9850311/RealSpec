package minimart.domain;

import java.io.Serial;

/**
 * One of the contract's error codes, raised as an exception.
 *
 * <p>It is unchecked because it is an answer, not an accident: it travels from wherever the rule
 * was decided — a domain function, a use case, the redemption transaction — up to the one place
 * that renders it ({@code ApiExceptionHandler}), and every layer in between has nothing to add.
 * Thrown inside a transaction it also rolls that transaction back, which is exactly what every
 * refusal must do.
 *
 * <p>No stack trace is captured. These are expected outcomes on the request path (a wrong password
 * is not a bug), and walking the stack for each one would be cost with no reader.
 */
public final class DomainException extends RuntimeException {

  @Serial private static final long serialVersionUID = 1L;

  private final ErrorCode code;

  /** An exception carrying {@code code} and nothing else. */
  public DomainException(ErrorCode code) {
    this(code, null);
  }

  /**
   * An exception carrying {@code code}, and the lower-level failure that led to it for the log's
   * benefit. The cause never reaches the caller: the wire carries only the code.
   */
  public DomainException(ErrorCode code, Throwable cause) {
    super(code.name(), cause, false, false);
    this.code = code;
  }

  /** The contract code this exception stands for. */
  public ErrorCode code() {
    return code;
  }
}
