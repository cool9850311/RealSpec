package repository

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"path/filepath"

	"github.com/golang-migrate/migrate/v4"
	pgdriver "github.com/golang-migrate/migrate/v4/database/postgres"
	// file:// source driver, registered by importing it.
	_ "github.com/golang-migrate/migrate/v4/source/file"
)

// Migrate applies every migration in dir (DDL only, no seed data) and leaves
// the database at the latest version. Applying an already-current database is
// not an error.
//
// It is used by two callers on purpose: the service at boot when
// SCHEMA_AUTO_MIGRATE is true, and the godog runner's `run migration` step. The
// schema the tests assert against is therefore produced by the same files and
// the same code path as the schema production runs on.
func Migrate(ctx context.Context, db *sql.DB, dir string) error {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return fmt.Errorf("resolve migrations dir %q: %w", dir, err)
	}

	// A dedicated connection, released here: handing golang-migrate the *sql.DB
	// would let its Close() close the pool the service is about to serve from.
	conn, err := db.Conn(ctx)
	if err != nil {
		return fmt.Errorf("acquire migration connection: %w", err)
	}
	defer func() { _ = conn.Close() }()

	driver, err := pgdriver.WithConnection(ctx, conn, &pgdriver.Config{})
	if err != nil {
		return fmt.Errorf("build migration driver: %w", err)
	}

	m, err := migrate.NewWithDatabaseInstance("file://"+filepath.ToSlash(abs), "postgres", driver)
	if err != nil {
		return fmt.Errorf("open migrations at %s: %w", abs, err)
	}
	if err := m.Up(); err != nil && !errors.Is(err, migrate.ErrNoChange) {
		return fmt.Errorf("apply migrations from %s: %w", abs, err)
	}
	return nil
}
