package repository

import (
	"context"
	"database/sql"
	"errors"
	"fmt"

	"minimart/internal/domain/entity"
)

// UserRepository reads the users table. The only writer of users.points is the
// redemption transaction in OrderRepository.
type UserRepository struct {
	db *sql.DB
}

// NewUserRepository returns a UserRepository backed by db.
func NewUserRepository(db *sql.DB) *UserRepository {
	return &UserRepository{db: db}
}

// FindByUsername returns the user with the given username, or ErrNotFound.
func (r *UserRepository) FindByUsername(ctx context.Context, username string) (*entity.User, error) {
	const q = `SELECT id, username, password_hash, role, points FROM users WHERE username = $1`

	var u entity.User
	err := r.db.QueryRowContext(ctx, q, username).
		Scan(&u.ID, &u.Username, &u.PasswordHash, &u.Role, &u.Points)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		return nil, ErrNotFound
	case err != nil:
		return nil, fmt.Errorf("find user by username: %w", err)
	}
	return &u, nil
}

// FindRoleByID returns just the role of the user with the given id, or
// ErrNotFound.
//
// It is a query of its own rather than a FindByID whose other columns are
// discarded, because it runs on every request to a role-guarded endpoint: the
// password hash of the caller has no business being read, let alone carried
// through the process, to answer a question about authority.
func (r *UserRepository) FindRoleByID(ctx context.Context, id int) (string, error) {
	const q = `SELECT role FROM users WHERE id = $1`

	var role string
	err := r.db.QueryRowContext(ctx, q, id).Scan(&role)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		return "", ErrNotFound
	case err != nil:
		return "", fmt.Errorf("find role by id: %w", err)
	}
	return role, nil
}

// FindByID returns the user with the given id, or ErrNotFound.
func (r *UserRepository) FindByID(ctx context.Context, id int) (*entity.User, error) {
	const q = `SELECT id, username, password_hash, role, points FROM users WHERE id = $1`

	var u entity.User
	err := r.db.QueryRowContext(ctx, q, id).
		Scan(&u.ID, &u.Username, &u.PasswordHash, &u.Role, &u.Points)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		return nil, ErrNotFound
	case err != nil:
		return nil, fmt.Errorf("find user by id: %w", err)
	}
	return &u, nil
}
