package dto

// ErrorResponse is every error body the API emits: a single flat object whose
// only key is a stable machine-readable code.
//
//	{ "error": "INSUFFICIENT_POINTS" }
type ErrorResponse struct {
	Error string `json:"error"`
}
