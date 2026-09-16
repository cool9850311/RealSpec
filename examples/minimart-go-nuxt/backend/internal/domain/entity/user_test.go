package entity_test

import (
	"errors"
	"testing"

	"minimart/internal/domain/entity"
)

// fixtureHash is the bcrypt (cost 10) hash of "secret123" that every seeded
// user in spec/bdd/** carries.
const fixtureHash = "$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2"

func TestVerifyPassword(t *testing.T) {
	t.Parallel()

	for _, tc := range []struct {
		name     string
		hash     string
		password string
		want     error
	}{
		{"the right password", fixtureHash, "secret123", nil},
		{"the wrong password", fixtureHash, "secret124", entity.ErrInvalidCredentials},
		{"an empty password", fixtureHash, "", entity.ErrInvalidCredentials},
		{"a column that never held a bcrypt hash", "secret123", "secret123", entity.ErrInvalidCredentials},
		{"an empty hash", "", "secret123", entity.ErrInvalidCredentials},
		{"a truncated hash", fixtureHash[:20], "secret123", entity.ErrInvalidCredentials},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			u := &entity.User{Username: "alice", PasswordHash: tc.hash}
			// A malformed hash must be a refusal, never a panic.
			if err := u.VerifyPassword(tc.password); !errors.Is(err, tc.want) {
				t.Fatalf("VerifyPassword = %v, want %v", err, tc.want)
			}
		})
	}
}
