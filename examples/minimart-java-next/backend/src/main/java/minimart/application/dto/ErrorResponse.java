package minimart.application.dto;

/**
 * Every error body the API emits: a single flat object whose only key is a stable machine-readable
 * code.
 *
 * <pre>{ "error": "INSUFFICIENT_POINTS" }</pre>
 */
public record ErrorResponse(String error) {}
