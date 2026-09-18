/**
 * The application layer: it orchestrates repositories and domain rules and returns wire shapes. It
 * knows nothing about HTTP, cookies or JWTs — a session is a transport concern and lives in
 * infrastructure.
 *
 * <p>The use cases are plain classes with constructors; the Spring wiring is in {@code
 * minimart.infrastructure.config.ApplicationWiring}, the one place that decides what is built over
 * what.
 */
package minimart.application.usecase;
