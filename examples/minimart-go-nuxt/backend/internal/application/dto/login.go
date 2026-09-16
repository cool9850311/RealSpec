// Package dto holds the wire shapes of spec/openapi/openapi.yaml. Nothing here
// has behaviour: these types exist so the JSON the service emits is declared in
// one place and reviewed against the contract.
package dto

// LoginRequest is the body of POST /api/v1/auth/login.
type LoginRequest struct {
	Username string `json:"username"`
	Password string `json:"password"`
}

// LoginResponse is the 200 body of POST /api/v1/auth/login. It carries neither
// the password hash nor the token — the token is only ever a cookie.
type LoginResponse struct {
	Username string `json:"username"`
	Role     string `json:"role"`
}

// MeResponse is the 200 body of GET /api/v1/me.
type MeResponse struct {
	Username string `json:"username"`
	Role     string `json:"role"`
	Points   int    `json:"points"`
}
