package minimart.domain;

/**
 * The stable error codes of the API contract (spec/openapi/openapi.yaml, schema {@code Error}). The
 * code IS the error: the human text belongs to the front end's i18n catalogue, never to the API, so
 * there is nothing else to carry.
 *
 * <p>{@code INTERNAL_ERROR} is deliberately not a member. It is not a domain outcome but the
 * absence of one — a defect in this service — and it is produced only by the transport layer when
 * something that is not a {@link DomainException} escapes.
 */
public enum ErrorCode {
  /** Malformed JSON, or a field that fails validation. */
  INVALID_REQUEST,

  /**
   * Unknown username or wrong password. The two are deliberately indistinguishable so the endpoint
   * does not disclose which usernames exist.
   */
  INVALID_CREDENTIALS,

  /** Missing, malformed or expired token cookie, or one whose subject names no users row. */
  UNAUTHENTICATED,

  /**
   * The caller is authenticated and the role stored on their users row is not the one the endpoint
   * requires. It is deliberately distinguishable from {@link #UNAUTHENTICATED}: "log in again" and
   * "this is not yours" are different remedies, and answering the second with the first sends a
   * caller round a loop that cannot end.
   */
  FORBIDDEN,

  /**
   * No such product, or its active flag is false. An inactive product does not exist as far as a
   * non-admin caller is concerned.
   */
  PRODUCT_NOT_FOUND,

  /** Balance below the product's cost_points. */
  INSUFFICIENT_POINTS,

  /** The product's stock is 0. */
  OUT_OF_STOCK
}
