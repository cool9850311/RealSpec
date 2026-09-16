package entity

import "golang.org/x/crypto/bcrypt"

// Roles. 'admin' may list every user's orders (GET /api/v1/admin/orders);
// 'guest' may not. The role that decides is the one on the users row, read per
// request — never the role claim carried in the token, which is a statement
// about the moment the token was issued and is not revocable.
const (
	RoleGuest = "guest"
	RoleAdmin = "admin"
)

// User is a row of the users table. PasswordHash never leaves the process.
type User struct {
	ID           int
	Username     string
	PasswordHash string
	Role         string
	Points       int
}

// VerifyPassword compares plain against the stored bcrypt hash.
//
// Every failure — a wrong password, a truncated hash, a column that never held
// a bcrypt hash at all — is reported as ErrInvalidCredentials and none of them
// panics: bcrypt.CompareHashAndPassword rejects a malformed hash with an error
// rather than by indexing into it.
func (u *User) VerifyPassword(plain string) error {
	if bcrypt.CompareHashAndPassword([]byte(u.PasswordHash), []byte(plain)) != nil {
		return ErrInvalidCredentials
	}
	return nil
}
