/**
 * The only place that speaks SQL. Every query is written out here, in the same text as the Go
 * service's repositories (only the placeholders differ: JDBC's {@code ?} for PostgreSQL's {@code
 * $n}); nothing above this layer knows that PostgreSQL exists.
 *
 * <p>A lookup that matches no row is an empty {@link java.util.Optional} (or, inside the redemption
 * transaction, a {@link minimart.infrastructure.repository.RowNotFoundException}). Mapping it onto
 * one of the API's error codes is the caller's decision, because the same missing row means
 * INVALID_CREDENTIALS during login and UNAUTHENTICATED when a token names a user who has since been
 * deleted.
 */
package minimart.infrastructure.repository;
