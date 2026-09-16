// Package config turns the process environment into one immutable struct.
// This file is the service's whole configuration surface — spec/spec.md points
// here rather than listing the names again. There are no others, and nothing
// reads os.Getenv outside this file.
package config

import (
	"fmt"
	"os"
	"strconv"
)

// Config is the whole configuration surface of the service.
type Config struct {
	// Port is the TCP port the HTTP server listens on. PORT, default 8080.
	Port string
	// DBDSN is the PostgreSQL connection string. DB_DSN, required.
	DBDSN string
	// JWTSecret is the HS256 signing key. JWT_SECRET, required.
	JWTSecret string
	// SchemaAutoMigrate runs the migrations at boot. SCHEMA_AUTO_MIGRATE,
	// default false.
	SchemaAutoMigrate bool
	// FrontendOrigin is the single origin CORS allows, or "*" to reflect
	// whatever origin asked. FRONTEND_ORIGIN, default empty, which sends no
	// CORS headers at all (correct for a same-origin deployment behind the
	// reverse proxy).
	FrontendOrigin string
	// CookieSecure adds Secure to the token cookie. COOKIE_SECURE, default
	// false; true everywhere the proxy terminates TLS.
	CookieSecure bool
	// MigrationsDir is where Migrate reads *.sql from. It is not an environment
	// variable: the Dockerfile puts the files next to the binary and the layout
	// is part of the image, not of the deployment.
	MigrationsDir string
}

// Load reads the environment and validates it. Every failure is reported here,
// at boot, rather than as a surprise on the first request that needs the value.
func Load() (*Config, error) {
	cfg := &Config{
		Port:           envOr("PORT", "8080"),
		DBDSN:          os.Getenv("DB_DSN"),
		JWTSecret:      os.Getenv("JWT_SECRET"),
		FrontendOrigin: os.Getenv("FRONTEND_ORIGIN"),
		MigrationsDir:  "migrations",
	}

	if cfg.DBDSN == "" {
		return nil, fmt.Errorf("DB_DSN is required")
	}
	if cfg.JWTSecret == "" {
		return nil, fmt.Errorf("JWT_SECRET is required")
	}

	var err error
	if cfg.SchemaAutoMigrate, err = envBool("SCHEMA_AUTO_MIGRATE", false); err != nil {
		return nil, err
	}
	if cfg.CookieSecure, err = envBool("COOKIE_SECURE", false); err != nil {
		return nil, err
	}
	return cfg, nil
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func envBool(key string, fallback bool) (bool, error) {
	raw := os.Getenv(key)
	if raw == "" {
		return fallback, nil
	}
	v, err := strconv.ParseBool(raw)
	if err != nil {
		return false, fmt.Errorf("%s must be a boolean, got %q", key, raw)
	}
	return v, nil
}
