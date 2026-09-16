// Command minimart is the backend of the RealSpec reference application:
// login, product list, points redemption.
package main

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/gin-gonic/gin"
	_ "github.com/lib/pq"

	"minimart/internal/infrastructure/config"
	"minimart/internal/infrastructure/repository"
	"minimart/internal/infrastructure/router"
)

// dbWaitTimeout is how long the process waits for PostgreSQL to accept
// connections before giving up. Container start order is not guaranteed, so
// "not up yet" is an expected state for the first few hundred milliseconds.
const dbWaitTimeout = 60 * time.Second

// shutdownTimeout is how long in-flight requests get to finish after a signal.
const shutdownTimeout = 10 * time.Second

func main() {
	if err := run(); err != nil {
		log.Fatalf("minimart: %v", err)
	}
}

func run() error {
	cfg, err := config.Load()
	if err != nil {
		return err
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	db, err := openDB(ctx, cfg.DBDSN)
	if err != nil {
		return err
	}
	defer func() {
		if cerr := db.Close(); cerr != nil {
			log.Printf("minimart: close database: %v", cerr)
		}
	}()

	if cfg.SchemaAutoMigrate {
		if err := repository.Migrate(ctx, db, cfg.MigrationsDir); err != nil {
			return err
		}
		log.Printf("minimart: schema migrated from %s", cfg.MigrationsDir)
	}

	gin.SetMode(gin.ReleaseMode)
	srv := &http.Server{
		Addr:              ":" + cfg.Port,
		Handler:           router.New(cfg, db),
		ReadHeaderTimeout: 10 * time.Second,
	}

	errCh := make(chan error, 1)
	go func() {
		log.Printf("minimart: listening on :%s", cfg.Port)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- fmt.Errorf("listen: %w", err)
			return
		}
		errCh <- nil
	}()

	select {
	case err := <-errCh:
		return err
	case <-ctx.Done():
		log.Printf("minimart: shutting down")
		shutdownCtx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
		defer cancel()
		if err := srv.Shutdown(shutdownCtx); err != nil {
			return fmt.Errorf("shutdown: %w", err)
		}
		return <-errCh
	}
}

// openDB opens the pool and waits until PostgreSQL answers, so that the HTTP
// port opens only once the service can actually serve.
func openDB(ctx context.Context, dsn string) (*sql.DB, error) {
	db, err := sql.Open("postgres", dsn)
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}
	db.SetMaxOpenConns(20)
	db.SetMaxIdleConns(10)
	db.SetConnMaxLifetime(time.Hour)

	waitCtx, cancel := context.WithTimeout(ctx, dbWaitTimeout)
	defer cancel()
	for {
		if err := db.PingContext(waitCtx); err == nil {
			return db, nil
		} else if waitCtx.Err() != nil {
			_ = db.Close()
			return nil, fmt.Errorf("database not reachable within %s: %w", dbWaitTimeout, err)
		}
		select {
		case <-waitCtx.Done():
			_ = db.Close()
			return nil, fmt.Errorf("database not reachable within %s", dbWaitTimeout)
		case <-time.After(200 * time.Millisecond):
		}
	}
}
