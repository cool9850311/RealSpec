// Package usecase is the application layer: it orchestrates repositories and
// domain rules and returns wire shapes. It knows nothing about gin, cookies or
// JWTs — a session is a transport concern and lives in infrastructure.
package usecase

import (
	"context"
	"errors"

	"golang.org/x/crypto/bcrypt"

	"minimart/internal/application/dto"
	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/repository"
)

// dummyHash is a valid bcrypt hash of a value nobody knows. It is compared
// against when the username does not exist, so that "no such user" costs the
// same time as "wrong password" and the endpoint does not leak which usernames
// are registered through its latency after refusing to leak it through its
// status code.
const dummyHash = "$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2"

// AuthUsecase verifies credentials and reads the caller's own profile.
type AuthUsecase struct {
	users *repository.UserRepository
}

// NewAuthUsecase returns an AuthUsecase over users.
func NewAuthUsecase(users *repository.UserRepository) *AuthUsecase {
	return &AuthUsecase{users: users}
}

// Authenticate returns the user whose password matches, or
// ErrInvalidCredentials. An unknown username and a wrong password are the same
// answer, deliberately.
func (uc *AuthUsecase) Authenticate(ctx context.Context, username, password string) (*entity.User, error) {
	user, err := uc.users.FindByUsername(ctx, username)
	switch {
	case errors.Is(err, repository.ErrNotFound):
		_ = bcrypt.CompareHashAndPassword([]byte(dummyHash), []byte(password))
		return nil, entity.ErrInvalidCredentials
	case err != nil:
		return nil, err
	}
	if err := user.VerifyPassword(password); err != nil {
		return nil, err
	}
	return user, nil
}

// RoleOf returns the role stored on the caller's users row.
//
// It exists so that authorisation is answered from the database rather than
// from the token's role claim. The claim states what was true when the token
// was issued; a JWT cannot be withdrawn, so a user demoted an hour ago still
// carries one saying otherwise for the rest of the day.
//
// A subject with no row is ErrUnauthenticated, for the same reason Me gives:
// the session is what has become invalid.
func (uc *AuthUsecase) RoleOf(ctx context.Context, userID int) (string, error) {
	role, err := uc.users.FindRoleByID(ctx, userID)
	switch {
	case errors.Is(err, repository.ErrNotFound):
		return "", entity.ErrUnauthenticated
	case err != nil:
		return "", err
	}
	return role, nil
}

// Me returns the profile of the authenticated caller.
//
// A token whose subject no longer exists is UNAUTHENTICATED, not 404: the
// session is the thing that is invalid, and the caller's remedy is to log in
// again rather than to look for a different user.
func (uc *AuthUsecase) Me(ctx context.Context, userID int) (*dto.MeResponse, error) {
	user, err := uc.users.FindByID(ctx, userID)
	switch {
	case errors.Is(err, repository.ErrNotFound):
		return nil, entity.ErrUnauthenticated
	case err != nil:
		return nil, err
	}
	return &dto.MeResponse{
		Username: user.Username,
		Role:     user.Role,
		Points:   user.Points,
	}, nil
}
