// Package entity holds minimart's domain types and the rules that are true of
// them regardless of transport or storage.
package entity

// CodeError is one of the stable error codes of the API contract
// (spec/openapi/openapi.yaml, schema Error). The code IS the error: the human text
// belongs to the front end's i18n catalogue, never to the API, so there is
// nothing else to carry.
//
// The values are comparable, so errors.Is works on them directly.
type CodeError string

// Error implements the error interface.
func (e CodeError) Error() string { return string(e) }

const (
	// ErrInvalidRequest — malformed JSON, or a field that fails validation.
	ErrInvalidRequest CodeError = "INVALID_REQUEST"
	// ErrInvalidCredentials — unknown username or wrong password. The two are
	// deliberately indistinguishable so the endpoint does not disclose which
	// usernames exist.
	ErrInvalidCredentials CodeError = "INVALID_CREDENTIALS"
	// ErrUnauthenticated — missing, malformed or expired token cookie, or one
	// whose subject names no users row.
	ErrUnauthenticated CodeError = "UNAUTHENTICATED"
	// ErrForbidden — the caller is authenticated and the role stored on their
	// users row is not the one the endpoint requires. It is deliberately
	// distinguishable from ErrUnauthenticated: "log in again" and "this is not
	// yours" are different remedies, and answering the second with the first
	// sends a caller round a loop that cannot end.
	ErrForbidden CodeError = "FORBIDDEN"
	// ErrProductNotFound — no such product, or its active flag is false. An
	// inactive product does not exist as far as a non-admin caller is concerned.
	ErrProductNotFound CodeError = "PRODUCT_NOT_FOUND"
	// ErrInsufficientPoints — balance below the product's cost_points.
	ErrInsufficientPoints CodeError = "INSUFFICIENT_POINTS"
	// ErrOutOfStock — the product's stock is 0.
	ErrOutOfStock CodeError = "OUT_OF_STOCK"
)
