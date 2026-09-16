// Package repository is the only place that speaks SQL. Every query is written
// out here; nothing above this layer knows that PostgreSQL exists.
package repository

import "errors"

// ErrNotFound is returned when a lookup matches no row. Mapping it onto one of
// the API's error codes is the caller's decision, because the same missing row
// means INVALID_CREDENTIALS during login and UNAUTHENTICATED when a token names
// a user who has since been deleted.
var ErrNotFound = errors.New("repository: no such row")
