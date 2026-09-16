// Package test is the godog runner for the API surface of spec/bdd/format.yml.
//
// Isolation, per PLAN.md §4: every scenario gets its own Docker network, its own
// PostgreSQL and its own copy of the service under test. Nothing is shared
// between scenarios except the service image, which is built once in TestMain.
// The measured cost of that isolation on this machine is a PostgreSQL cold start
// of well under a second, which is cheaper than the class of bug that leaking
// one scenario's rows into the next produces.
//
// This file holds everything the steps need: the container lifecycle, the HTTP
// client, the SQL helpers, and the partial JSON matcher. suite_test.go binds
// them to the step patterns.
//
// The client keeps no cookie jar. A scenario carries identity by writing
// "Cookie": "token={someVar}" into the request docstring, every request, with
// the token named by a `save response cookie` under a login the feature wrote
// out, or by one of the four pre-minted credentials this file puts in every
// scenario's bag. One mechanism, visible where it is used.
package test

import (
	"bufio"
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/cucumber/godog"
	"github.com/golang-jwt/jwt/v5"
	_ "github.com/lib/pq"
	"github.com/testcontainers/testcontainers-go"
	tcpostgres "github.com/testcontainers/testcontainers-go/modules/postgres"
	tcnetwork "github.com/testcontainers/testcontainers-go/network"
	"github.com/testcontainers/testcontainers-go/wait"

	"minimart/internal/infrastructure/repository"
)

const (
	// serviceImage is the tag TestMain builds backend/Dockerfile into, and the
	// image every per-scenario service container runs.
	serviceImage = "minimart-backend:godog"

	// postgresImage is the same image local/docker-compose.yml uses.
	postgresImage = "postgres:16-alpine"

	// How long to wait for the mapped PostgreSQL port to accept a connection.
	dbReadyTimeout      = 30 * time.Second
	dbReadyPollInterval = 100 * time.Millisecond

	dbName     = "minimart"
	dbUser     = "minimart"
	dbPassword = "minimart_secret"

	// jwtSecret is this suite's signing key. It is not the deployment's: a test
	// that passes only because it shares production's secret is not a test.
	//
	// It is also what makes the pre-minted credentials possible with no
	// test-only code in the service at all: the suite hands this value to the
	// container, so it already holds every input needed to sign a token the
	// service will accept. A backdoor token or a debug endpoint would buy the
	// same scenarios by putting the attacker's tools inside the artefact under
	// test.
	jwtSecret = "minimart-godog-secret"

	// wrongJWTSecret signs {tokenWrongKey}: a key no container is ever given,
	// so a token signed with it can only be rejected. Its content is irrelevant
	// as long as it differs from jwtSecret.
	wrongJWTSecret = "minimart-godog-not-the-secret"

	// signedTTL is the lifetime the service itself issues (24h), so a pre-minted
	// token is indistinguishable from one POST /api/v1/auth/login would have
	// issued except in the one field it deviates on.
	signedTTL = 24 * time.Hour

	// The identities the pre-minted credentials speak for. They are constants
	// rather than lookups because these tokens are minted before the Background
	// runs — there is no users table yet to read. Every API Background seeds the
	// fixture guest as users.id = 1 under this name, and none of them seeds
	// unknownUserID; spec/bdd/format.yml says so where a feature's reader is,
	// and the scenarios that turn on it assert the premise in SQL.
	fixtureGuestID       = "1"
	fixtureGuestUsername = "alice"
	fixtureGuestRole     = "guest"

	unknownUserID       = "4242"
	unknownUserUsername = "ghost"

	startupTimeout = 120 * time.Second
)

// migrationsDir is backend/migrations, resolved from this package's directory.
const migrationsDir = "../../migrations"

// buildContextDir is examples/minimart-go-nuxt, which is the build context
// local/docker-compose.yml uses (`context: ..`, `dockerfile: backend/Dockerfile`).
const buildContextDir = "../../.."

// contextVarPattern matches an unresolved {contextVar} left in a docstring after
// substitution. Finding one is a missing `save response body field`, and saying
// so beats letting PostgreSQL report a syntax error three steps later.
var contextVarPattern = regexp.MustCompile(`\{[a-z][a-zA-Z0-9]*\}`)

// ScenarioCtx is one scenario's world: its containers, its database handle, its
// HTTP client and the state the assertion steps read.
type ScenarioCtx struct {
	network *testcontainers.DockerNetwork
	pgC     *tcpostgres.PostgresContainer
	svcC    testcontainers.Container

	// db is a direct connection to this scenario's PostgreSQL, used by the
	// seeding and assertion steps. It bypasses the service on purpose: an
	// assertion that went through the API could not catch an API that lies.
	db *sql.DB

	// baseURL is this scenario's service, reached on its mapped port.
	baseURL string

	// httpClient holds no cookie jar — nothing is carried from one request to
	// the next but the context bag — and does NOT follow redirects, so a 3xx is
	// observable as a status rather than silently resolved into the status of
	// somewhere else.
	httpClient *http.Client

	lastResp *http.Response
	lastBody []byte

	// responseSet is what the last concurrent request step recorded. Exactly one
	// of it and lastResp is ever set: a step records either one response or a
	// set, and each clears the other, so the two vocabularies cannot be mixed.
	responseSet []recordedResponse

	// vars is the context bag: {varName} tokens resolved in paths, JSON and SQL.
	vars map[string]string
}

// recordedResponse is one caller's answer in a concurrent set: the status, and
// the body kept for the distribution a failed assertion prints.
type recordedResponse struct {
	status int
	body   []byte
}

// startInfra brings up this scenario's network, database and service. It is
// called from sc.Before, once per scenario.
// pingUntilReady waits for the mapped port to accept a connection.
func pingUntilReady(ctx context.Context, db *sql.DB) error {
	deadline := time.Now().Add(dbReadyTimeout)
	var last error
	for {
		if last = db.PingContext(ctx); last == nil {
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("database did not accept a connection within %s: %w", dbReadyTimeout, last)
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(dbReadyPollInterval):
		}
	}
}

func (s *ScenarioCtx) startInfra(ctx context.Context) error {
	s.vars = map[string]string{}
	if err := s.mintCredentials(); err != nil {
		return err
	}

	s.httpClient = &http.Client{
		Timeout: 30 * time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}

	net, err := tcnetwork.New(ctx)
	if err != nil {
		return fmt.Errorf("create network: %w", err)
	}
	s.network = net

	// PostgreSQL with durability turned off and its data directory on tmpfs.
	// This database lives for one scenario and is thrown away; fsync buys
	// nothing here and costs the startup time that makes per-scenario
	// isolation affordable in the first place.
	pgC, err := tcpostgres.Run(ctx, postgresImage,
		tcpostgres.WithDatabase(dbName),
		tcpostgres.WithUsername(dbUser),
		tcpostgres.WithPassword(dbPassword),
		testcontainers.WithCmd("postgres",
			"-c", "fsync=off",
			"-c", "full_page_writes=off",
			"-c", "synchronous_commit=off",
		),
		testcontainers.WithTmpfs(map[string]string{"/var/lib/postgresql/data": "rw"}),
		testcontainers.WithWaitStrategy(
			// The entrypoint starts a temporary server to initialise the
			// cluster before the real one, so the message appears twice.
			wait.ForLog("database system is ready to accept connections").
				WithOccurrence(2).
				WithStartupTimeout(startupTimeout),
		),
		tcnetwork.WithNetwork([]string{"postgres"}, net),
	)
	if err != nil {
		return fmt.Errorf("start postgres: %w", err)
	}
	s.pgC = pgC

	hostDSN, err := pgC.ConnectionString(ctx, "sslmode=disable")
	if err != nil {
		return fmt.Errorf("postgres connection string: %w", err)
	}
	db, err := sql.Open("postgres", hostDSN)
	if err != nil {
		return fmt.Errorf("open database: %w", err)
	}
	// Retry rather than ping once. The container's wait strategy proves the
	// server is listening INSIDE the container; it says nothing about the host
	// port forward, which is not always routable at that instant. A single ping
	// turns that gap into a scenario that fails before its first step.
	if err := pingUntilReady(ctx, db); err != nil {
		return err
	}
	s.db = db

	// The service under test, configured the way the contract names its
	// environment. SCHEMA_AUTO_MIGRATE is false because `run migration` is a
	// step of the feature: the schema is created by the Background, in the
	// open, not as a side effect of a container booting.
	// COOKIE_SECURE is false because this suite talks plain HTTP directly to
	// the service; the Secure attribute is exercised by the e2e stack, which
	// has a TLS-terminating proxy in front of it.
	svcC, err := testcontainers.GenericContainer(ctx, testcontainers.GenericContainerRequest{
		ContainerRequest: testcontainers.ContainerRequest{
			Image:        serviceImage,
			ExposedPorts: []string{"8080/tcp"},
			Networks:     []string{net.Name},
			Env: map[string]string{
				"PORT":                "8080",
				"DB_DSN":              fmt.Sprintf("postgres://%s:%s@postgres:5432/%s?sslmode=disable", dbUser, dbPassword, dbName),
				"JWT_SECRET":          jwtSecret,
				"SCHEMA_AUTO_MIGRATE": "false",
				"FRONTEND_ORIGIN":     "*",
				"COOKIE_SECURE":       "false",
			},
			// Any status proves the HTTP server is listening, which is all
			// that is being waited for: the schema does not exist yet, so a
			// route that reads the database is entitled to fail at this point.
			WaitingFor: wait.ForHTTP("/api/v1/products").
				WithPort("8080/tcp").
				WithStatusCodeMatcher(func(int) bool { return true }).
				WithStartupTimeout(startupTimeout),
		},
		Started: true,
	})
	if err != nil {
		return fmt.Errorf("start service: %w", err)
	}
	s.svcC = svcC

	host, err := svcC.Host(ctx)
	if err != nil {
		return fmt.Errorf("service host: %w", err)
	}
	port, err := svcC.MappedPort(ctx, "8080")
	if err != nil {
		return fmt.Errorf("service port: %w", err)
	}
	s.baseURL = fmt.Sprintf("http://%s:%s", host, port.Port())
	return nil
}

// stopInfra tears the scenario down. Called from sc.After, including after a
// failure, so a red scenario leaves no containers behind either.
func (s *ScenarioCtx) stopInfra(ctx context.Context) {
	if s.svcC != nil {
		_ = s.svcC.Terminate(ctx)
	}
	if s.db != nil {
		_ = s.db.Close()
	}
	if s.pgC != nil {
		_ = s.pgC.Terminate(ctx)
	}
	if s.network != nil {
		_ = s.network.Remove(ctx)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared steps: infrastructure setup
// ─────────────────────────────────────────────────────────────────────────────

// runMigration implements `run migration`.
//
// It applies backend/migrations to this scenario's database through the very
// function the service calls when SCHEMA_AUTO_MIGRATE is true, so the schema the
// features assert against is the schema production gets.
func (s *ScenarioCtx) runMigration(ctx context.Context) error {
	dir, err := filepath.Abs(migrationsDir)
	if err != nil {
		return fmt.Errorf("resolve migrations dir: %w", err)
	}
	if err := repository.Migrate(ctx, s.db, dir); err != nil {
		return fmt.Errorf("run migration: %w", err)
	}
	return nil
}

// execPostgres implements `in PostgreSQL:` — the docstring is run as one raw
// SQL batch outside any explicit transaction.
//
// Afterwards every SERIAL sequence is set past the largest id in its table. The
// features seed rows with explicit ids (`INSERT INTO products (id, …)`), which
// does not advance the sequence, so without this the first row the API inserts
// would collide with a seeded id — a failure that says nothing about the code
// under test.
func (s *ScenarioCtx) execPostgres(ctx context.Context, doc *godog.DocString) error {
	stmt, err := s.resolve(doc.Content)
	if err != nil {
		return err
	}
	if _, err := s.db.ExecContext(ctx, stmt); err != nil {
		return fmt.Errorf("exec SQL failed: %w\nSQL: %s", err, stmt)
	}
	if _, err := s.db.ExecContext(ctx, resetSequencesSQL); err != nil {
		return fmt.Errorf("reset sequences failed: %w", err)
	}
	return nil
}

const resetSequencesSQL = `
DO $$
DECLARE r RECORD;
BEGIN
    FOR r IN
        SELECT c.table_name, c.column_name,
               pg_get_serial_sequence(c.table_name, c.column_name) AS seq
          FROM information_schema.columns c
         WHERE c.table_schema = 'public'
           AND c.column_default LIKE 'nextval%'
    LOOP
        CONTINUE WHEN r.seq IS NULL;
        -- setval(seq, n, false) makes nextval() return n.
        EXECUTE format(
            'SELECT setval(%L, COALESCE((SELECT MAX(%I)+1 FROM %I), 1), false)',
            r.seq, r.column_name, r.table_name
        );
    END LOOP;
END $$;`

// postgresQueryReturns implements `in PostgreSQL query returns <n> rows:`.
//
// 0 asserts no matching record exists, 1 asserts exactly one, and n > 1 asserts
// at least n — the semantics format.yml documents.
func (s *ScenarioCtx) postgresQueryReturns(ctx context.Context, expected int, doc *godog.DocString) error {
	query, err := s.resolve(doc.Content)
	if err != nil {
		return err
	}
	count, err := s.countRows(ctx, query)
	if err != nil {
		return err
	}
	switch {
	case expected == 0 && count != 0:
		return fmt.Errorf("expected 0 rows, got %d\nSQL: %s", count, query)
	case expected == 1 && count != 1:
		return fmt.Errorf("expected exactly 1 row, got %d\nSQL: %s", count, query)
	case expected > 1 && count < expected:
		return fmt.Errorf("expected at least %d rows, got %d\nSQL: %s", expected, count, query)
	}
	return nil
}

func (s *ScenarioCtx) countRows(ctx context.Context, query string) (int, error) {
	rows, err := s.db.QueryContext(ctx, query)
	if err != nil {
		return 0, fmt.Errorf("query failed: %w\nSQL: %s", err, query)
	}
	defer rows.Close()

	count := 0
	for rows.Next() {
		count++
	}
	if err := rows.Err(); err != nil {
		return 0, fmt.Errorf("iterating rows: %w\nSQL: %s", err, query)
	}
	return count, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// The pre-minted credentials
//
// Four context variables exist in every scenario before its first step runs,
// documented for the features in spec/bdd/format.yml. Each holds a complete
// HS256 JWT that is what POST /api/v1/auth/login would have issued for the
// fixture guest, deviating in exactly one field — and the deviation is what the
// scenario carrying it tests.
//
// The suite can sign them because the suite is what sets JWT_SECRET on the
// container: no cooperation is needed from the service, and none is given. The
// alternatives all fail the same way — a backdoor token, a test-only endpoint
// or a debug flag would move the attacker's tools inside the artefact being
// defended, and the suite would then be evidence about a build nobody deploys.
//
// They are values in the bag and nothing more. None of them authenticates
// anything, none of them issues a request, and none of them moves the response
// the assertion steps refer to. A request is made with one only when its own
// docstring writes "Cookie": "token={tokenWhatever}".
//
// There is no step that mints a fifth, on purpose: every deviation this suite
// tests is written here, once, where it can be read alongside the other three
// rather than reinvented per feature.
// ─────────────────────────────────────────────────────────────────────────────

// mintCredentials fills the bag with the four pre-minted credentials. It is
// called as the bag is created, before the Background runs, so it cannot
// consult the database — which is why the identities below are constants and
// why the features that turn on them assert the matching row in SQL.
func (s *ScenarioCtx) mintCredentials() error {
	now := time.Now()

	// The genuine session all four are derived from: the fixture guest, signed
	// with the key this scenario's container is given, valid for as long as the
	// service's own tokens are. Three of them change one claim; the fourth
	// changes nothing but the key it is signed with, which is what makes the
	// signature the only possible reason to refuse it.
	genuine := func() jwt.MapClaims {
		return jwt.MapClaims{
			"sub":      fixtureGuestID,
			"username": fixtureGuestUsername,
			"role":     fixtureGuestRole,
			"iat":      jwt.NewNumericDate(now),
			"exp":      jwt.NewNumericDate(now.Add(signedTTL)),
		}
	}

	// Expired: issued a full lifetime ago and one minute past its end. Signed
	// correctly, so nothing but the clock can be the reason it is refused.
	expired := genuine()
	expiredAt := now.Add(-time.Minute)
	expired["iat"] = jwt.NewNumericDate(expiredAt.Add(-signedTTL))
	expired["exp"] = jwt.NewNumericDate(expiredAt)

	// A user who is gone: a sub no Background seeds, under a username no
	// Background seeds either, so neither lookup can accidentally succeed.
	unknown := genuine()
	unknown["sub"] = unknownUserID
	unknown["username"] = unknownUserUsername

	// The privilege-escalation case: valid JWT, real key, unexpired, claiming a
	// role the users row does not grant.
	claimsAdmin := genuine()
	claimsAdmin["role"] = "admin"

	for _, credential := range []struct {
		varName string
		claims  jwt.MapClaims
		secret  string
	}{
		{"tokenExpired", expired, jwtSecret},
		{"tokenWrongKey", genuine(), wrongJWTSecret},
		{"tokenUnknownUser", unknown, jwtSecret},
		{"tokenClaimsAdmin", claimsAdmin, jwtSecret},
	} {
		token, err := jwt.NewWithClaims(jwt.SigningMethodHS256, credential.claims).
			SignedString([]byte(credential.secret))
		if err != nil {
			return fmt.Errorf("mint {%s}: %w", credential.varName, err)
		}
		s.vars[credential.varName] = token
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// API steps: the request
// ─────────────────────────────────────────────────────────────────────────────

// requestEnvelope is the docstring of an http_request step. Headers and body are
// both optional and nothing else is allowed, which DisallowUnknownFields turns
// from a convention into a rule.
type requestEnvelope struct {
	Headers map[string]string `json:"headers"`
	Body    json.RawMessage   `json:"body"`
}

// bodyReader turns an envelope's body into the request body to send. An absent
// body, {} or null all mean "send none", so a GET that needs neither headers nor
// body can still carry the {} docstring the grammar requires.
func bodyReader(raw json.RawMessage) io.Reader {
	trimmed := strings.TrimSpace(string(raw))
	if trimmed == "" || trimmed == "{}" || trimmed == "null" {
		return nil
	}
	return strings.NewReader(trimmed)
}

// buildRequest assembles one request against this scenario's service.
//
// Building is separate from sending because the concurrent step has to do all
// of its building before any of its sending: a caller that is still assembling
// a request is not yet at the starting gate.
//
// body may be nil, in which case no request body and no Content-Type are sent.
// An entry in headers overrides the Content-Type this sets.
func (s *ScenarioCtx) buildRequest(ctx context.Context, method, path string, headers map[string]string, body io.Reader) (*http.Request, error) {
	req, err := http.NewRequestWithContext(ctx, method, s.baseURL+path, body)
	if err != nil {
		return nil, fmt.Errorf("build request: %w", err)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	return req, nil
}

// readBody reads a response body in full and closes it.
func readBody(resp *http.Response) ([]byte, error) {
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("read response body: %w", err)
	}
	return body, nil
}

// send builds one request and performs it on the scenario's client. It records
// nothing; whether the result becomes the subject of the assertion steps is the
// caller's decision.
func (s *ScenarioCtx) send(ctx context.Context, method, path string, headers map[string]string, body io.Reader) (*http.Response, []byte, error) {
	req, err := s.buildRequest(ctx, method, path, headers, body)
	if err != nil {
		return nil, nil, err
	}
	resp, err := s.httpClient.Do(req)
	if err != nil {
		return nil, nil, fmt.Errorf("%s %s failed: %w", method, path, err)
	}
	responseBody, err := readBody(resp)
	if err != nil {
		return nil, nil, err
	}
	return resp, responseBody, nil
}

// httpRequest implements `<METHOD> <path>:`.
//
// The client holds no cookie jar, so this request sends exactly the headers its
// own docstring writes. A request that is made as somebody says so itself, with
// "Cookie": "token={someVar}" — the whole docstring goes through resolve, so a
// token saved by saveResponseCookie, and a pre-minted one, are substituted there
// like anything else in the bag.
//
// Transport errors fail the step. Any HTTP status, 5xx included, is recorded and
// left to `response status is` to assert.
func (s *ScenarioCtx) httpRequest(ctx context.Context, method, path string, doc *godog.DocString) error {
	path, err := s.resolve(path)
	if err != nil {
		return err
	}
	content, err := s.resolve(doc.Content)
	if err != nil {
		return err
	}

	dec := json.NewDecoder(strings.NewReader(content))
	dec.DisallowUnknownFields()
	var envelope requestEnvelope
	if err := dec.Decode(&envelope); err != nil {
		return fmt.Errorf("request docstring must be a JSON object with only \"headers\" and \"body\": %w", err)
	}

	resp, responseBody, err := s.send(ctx, method, path, envelope.Headers, bodyReader(envelope.Body))
	if err != nil {
		return err
	}
	s.lastResp = resp
	s.lastBody = responseBody
	// One response is on record again; the concurrent set is not.
	s.responseSet = nil
	return nil
}

// concurrentDeadline bounds one caller of a concurrent burst, matching the
// scenario client's own timeout.
const concurrentDeadline = 30 * time.Second

// httpRequestConcurrent implements `<METHOD> <path> is called concurrently:`.
//
// Every request is built and every connection opened before any caller is
// released, and the request is written onto that connection rather than handed
// to an http.Client: the gate opens onto a write syscall and nothing else. The
// race window is the winner's transaction, under a millisecond against a tmpfs
// database, so a caller still shaking hands with Docker's published port
// arrives after it has closed. The raw write costs nothing — the scenario's
// client exists to keep no cookie jar and to refuse redirects, which a bare
// connection does by having neither.
//
// The other half is the feature's: the service takes a database connection per
// in-flight request, and one being opened is one not racing, so a scenario
// warms the pool with a concurrent read first. With both halves a removed
// FOR UPDATE was caught in 3 runs of 3; with neither, 0 of 3.
//
// The outcome is a SET and the single response is cleared, so an assertion that
// names one fails through requireResponse rather than reading a stale answer.
func (s *ScenarioCtx) httpRequestConcurrent(ctx context.Context, method, path string, doc *godog.DocString) error {
	path, err := s.resolve(path)
	if err != nil {
		return err
	}
	content, err := s.resolve(doc.Content)
	if err != nil {
		return err
	}

	dec := json.NewDecoder(strings.NewReader(content))
	dec.DisallowUnknownFields()
	var envelopes []requestEnvelope
	if err := dec.Decode(&envelopes); err != nil {
		return fmt.Errorf("concurrent request docstring must be a JSON array of objects with only \"headers\" and \"body\": %w", err)
	}
	if len(envelopes) < 2 {
		return fmt.Errorf("concurrent request docstring lists %d caller(s); a race needs at least 2", len(envelopes))
	}

	s.lastResp, s.lastBody, s.responseSet = nil, nil, nil

	address, err := url.Parse(s.baseURL)
	if err != nil {
		return fmt.Errorf("parse service URL %q: %w", s.baseURL, err)
	}

	requests := make([]*http.Request, len(envelopes))
	conns := make([]net.Conn, len(envelopes))
	defer func() {
		for _, conn := range conns {
			if conn != nil {
				_ = conn.Close()
			}
		}
	}()

	var dialer net.Dialer
	for i, envelope := range envelopes {
		req, err := s.buildRequest(ctx, method, path, envelope.Headers, bodyReader(envelope.Body))
		if err != nil {
			return fmt.Errorf("caller %d of %d: %w", i+1, len(envelopes), err)
		}
		// One request per connection: the answer ends the conversation.
		req.Close = true
		requests[i] = req

		conn, err := dialer.DialContext(ctx, "tcp", address.Host)
		if err != nil {
			return fmt.Errorf("caller %d of %d: connect to %s: %w", i+1, len(envelopes), address.Host, err)
		}
		if err := conn.SetDeadline(time.Now().Add(concurrentDeadline)); err != nil {
			return fmt.Errorf("caller %d of %d: set deadline: %w", i+1, len(envelopes), err)
		}
		conns[i] = conn
	}

	responses := make([]recordedResponse, len(envelopes))
	errs := make([]error, len(envelopes))

	// ready counts the callers parked at the gate; start is the gate. Closing a
	// channel wakes every receiver at once.
	var ready, done sync.WaitGroup
	start := make(chan struct{})

	for i := range requests {
		ready.Add(1)
		done.Add(1)
		go func(i int) {
			defer done.Done()
			ready.Done()
			<-start

			if err := requests[i].Write(conns[i]); err != nil {
				errs[i] = fmt.Errorf("write request: %w", err)
				return
			}
			resp, err := http.ReadResponse(bufio.NewReader(conns[i]), requests[i])
			if err != nil {
				errs[i] = fmt.Errorf("read response: %w", err)
				return
			}
			body, err := readBody(resp)
			if err != nil {
				errs[i] = err
				return
			}
			responses[i] = recordedResponse{status: resp.StatusCode, body: body}
		}(i)
	}

	ready.Wait()
	close(start)
	done.Wait()

	for i, err := range errs {
		if err != nil {
			return fmt.Errorf("caller %d of %d: %w", i+1, len(envelopes), err)
		}
	}
	s.responseSet = responses
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// API steps: the assertions
// ─────────────────────────────────────────────────────────────────────────────

// requireResponse is the guard every single-response assertion opens with.
//
// After a concurrent step there is no "the response" to assert, and the danger
// is not the absence — it is the response an earlier step left behind, which
// would let a status-only assertion pass against the wrong request entirely.
// So the set is named in the message and the step fails.
func (s *ScenarioCtx) requireResponse() error {
	switch {
	case s.lastResp != nil:
		return nil
	case s.responseSet != nil:
		return fmt.Errorf("the last HTTP step was concurrent and recorded %d responses, not one; "+
			"assert them with `exactly <n> responses are <status>`", len(s.responseSet))
	default:
		return fmt.Errorf("no response recorded: no HTTP step has run yet")
	}
}

// responseStatusIs implements `response status is <code>`.
func (s *ScenarioCtx) responseStatusIs(expected int) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	if s.lastResp.StatusCode != expected {
		// A wrong status is almost always explained by the body, so print it.
		return fmt.Errorf("expected HTTP %d, got %d\nBody: %s",
			expected, s.lastResp.StatusCode, s.lastBody)
	}
	return nil
}

// responseSetStatusCount implements `exactly <n> responses are <status>`.
//
// Exact, never "at least": one 201 among four callers is the whole claim a race
// makes, and an "at least" form would pass on the oversell it exists to catch.
func (s *ScenarioCtx) responseSetStatusCount(expected, status int) error {
	if s.responseSet == nil {
		return fmt.Errorf("no concurrent responses recorded: no `is called concurrently:` step has run yet")
	}
	count := 0
	for _, response := range s.responseSet {
		if response.status == status {
			count++
		}
	}
	if count != expected {
		return fmt.Errorf("expected exactly %d of the %d responses to be HTTP %d, got %d\nActual distribution:\n%s",
			expected, len(s.responseSet), status, count, s.statusDistribution())
	}
	return nil
}

// statusDistribution renders the recorded set as a count per status with one
// body each, so a failed expectation says what really happened.
func (s *ScenarioCtx) statusDistribution() string {
	counts := map[int]int{}
	sample := map[int]string{}
	for _, response := range s.responseSet {
		counts[response.status]++
		if _, seen := sample[response.status]; !seen {
			sample[response.status] = strings.TrimSpace(string(response.body))
		}
	}
	statuses := make([]int, 0, len(counts))
	for status := range counts {
		statuses = append(statuses, status)
	}
	sort.Ints(statuses)

	lines := make([]string, 0, len(statuses))
	for _, status := range statuses {
		lines = append(lines, fmt.Sprintf("  %d × HTTP %d, e.g. %s", counts[status], status, sample[status]))
	}
	return strings.Join(lines, "\n")
}

// responseBodyContains implements `response body contains:` — a partial match:
// only the keys present in the docstring are checked and extra keys in the
// response are ignored. Arrays are matched element-wise and their lengths must
// be equal, so a list assertion cannot pass against a longer list.
func (s *ScenarioCtx) responseBodyContains(doc *godog.DocString) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	content, err := s.resolve(doc.Content)
	if err != nil {
		return err
	}

	var expected any
	if err := json.Unmarshal([]byte(content), &expected); err != nil {
		return fmt.Errorf("expected JSON is not valid: %w", err)
	}
	var actual any
	if err := json.Unmarshal(s.lastBody, &actual); err != nil {
		return fmt.Errorf("response body is not valid JSON: %w\nBody: %s", err, s.lastBody)
	}

	if err := matchSubset(expected, actual, "$"); err != nil {
		return fmt.Errorf("%w\nActual body:\n%s", err, indentJSON(s.lastBody))
	}
	return nil
}

// responseBodyDoesNotContain implements `response body does not contain "<text>"`.
//
// The check is on the raw body, at any nesting level, whether the text appears
// as a key or inside a value: proving that `token` is absent from a login
// response means absent, not merely "not a top-level key".
func (s *ScenarioCtx) responseBodyDoesNotContain(text string) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	body := string(s.lastBody)
	idx := strings.Index(body, text)
	if idx < 0 {
		return nil
	}
	start := max(0, idx-40)
	end := min(len(body), idx+len(text)+40)
	return fmt.Errorf("response body contains %q but must not: …%s…", text, body[start:end])
}

// responseHeaderContains implements
// `response header "<name>" contains "<substring>"`.
//
// The name is matched case-insensitively; the value is a case-sensitive
// substring. A header that occurs more than once — Set-Cookie does — passes if
// any occurrence contains the substring.
func (s *ScenarioCtx) responseHeaderContains(name, substring string) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	values := s.lastResp.Header.Values(name)
	if len(values) == 0 {
		present := make([]string, 0, len(s.lastResp.Header))
		for k := range s.lastResp.Header {
			present = append(present, k)
		}
		sort.Strings(present)
		return fmt.Errorf("response has no %q header; headers present: %s",
			name, strings.Join(present, ", "))
	}
	for _, v := range values {
		if strings.Contains(v, substring) {
			return nil
		}
	}
	return fmt.Errorf("no %q header contains %q; values: %q", name, substring, values)
}

// saveResponseBodyField implements
// `save response body field "<field>" as "<varName>"`.
//
// The value is stored as a string so the same variable interpolates into JSON
// and into SQL. Numbers are rendered without quotes or exponent (1 → "1").
func (s *ScenarioCtx) saveResponseBodyField(field, varName string) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	var body map[string]any
	if err := json.Unmarshal(s.lastBody, &body); err != nil {
		return fmt.Errorf("response body is not a JSON object: %w\nBody: %s", err, s.lastBody)
	}
	value, ok := body[field]
	if !ok {
		return fmt.Errorf("response body has no field %q\nBody: %s", field, indentJSON(s.lastBody))
	}
	if value == nil {
		return fmt.Errorf("response body field %q is null", field)
	}
	rendered, err := renderValue(value)
	if err != nil {
		return fmt.Errorf("field %q: %w", field, err)
	}
	s.vars[varName] = rendered
	return nil
}

// saveResponseCookie implements
// `save response cookie "<name>" as "<varName>"`.
//
// The other half of saveResponseBodyField: the same contract — produce a value,
// name it, change nothing else — for the part of a response that is not the
// body. It is how a scenario carries a session, because a login is written out
// as the ordinary request it is and the token is in the Set-Cookie header, not
// in the body the login answers with.
//
// Saving a token authenticates nothing: the client keeps no cookie jar, so a
// later request is made as somebody only when its own docstring writes the
// header.
func (s *ScenarioCtx) saveResponseCookie(name, varName string) error {
	if err := s.requireResponse(); err != nil {
		return err
	}
	for _, cookie := range s.lastResp.Cookies() {
		if cookie.Name != name {
			continue
		}
		// An empty value is how a session is CLEARED — the service expires the
		// session hint with exactly that. Saving it would name a credential
		// that authenticates nothing while every request below reads as though
		// it does, which is the one failure this step exists to make
		// impossible.
		if cookie.Value == "" {
			return fmt.Errorf("response set cookie %q to the empty string, which clears it rather than granting one", name)
		}
		s.vars[varName] = cookie.Value
		return nil
	}
	return fmt.Errorf("response set no %q cookie (Set-Cookie: %v)",
		name, s.lastResp.Header.Values("Set-Cookie"))
}

// renderValue turns a decoded JSON value into the string a context variable
// holds.
func renderValue(value any) (string, error) {
	switch v := value.(type) {
	case string:
		return v, nil
	case bool:
		return strconv.FormatBool(v), nil
	case float64:
		if v == float64(int64(v)) {
			return strconv.FormatInt(int64(v), 10), nil
		}
		return strconv.FormatFloat(v, 'f', -1, 64), nil
	default:
		encoded, err := json.Marshal(v)
		if err != nil {
			return "", fmt.Errorf("cannot render %T as a context variable: %w", value, err)
		}
		return string(encoded), nil
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Context variables and the partial JSON matcher
// ─────────────────────────────────────────────────────────────────────────────

// resolve substitutes every {varName} in text from the scenario's context bag.
// A token that survives substitution is an error: it means the feature reads a
// variable no step ever saved, and reporting that here beats letting it reach
// PostgreSQL as a literal.
func (s *ScenarioCtx) resolve(text string) (string, error) {
	for name, value := range s.vars {
		text = strings.ReplaceAll(text, "{"+name+"}", value)
	}
	if leftover := contextVarPattern.FindString(text); leftover != "" {
		return "", fmt.Errorf("unknown context variable %s (saved so far: %s)",
			leftover, strings.Join(s.savedVarNames(), ", "))
	}
	return text, nil
}

func (s *ScenarioCtx) savedVarNames() []string {
	names := make([]string, 0, len(s.vars))
	for name := range s.vars {
		names = append(names, name)
	}
	sort.Strings(names)
	if len(names) == 0 {
		return []string{"none"}
	}
	return names
}

// matchSubset reports whether actual satisfies expected under the partial-match
// rules of format.yml, naming the first path that does not.
//
//   - objects: every key of expected must exist in actual and match; extra keys
//     in actual are ignored.
//   - arrays: matched element-wise, lengths must be equal.
//   - "<non-null>": the field must be present and not JSON null; nothing else is
//     constrained.
//   - anything else: equality.
func matchSubset(expected, actual any, path string) error {
	if s, ok := expected.(string); ok && s == "<non-null>" {
		if actual == nil {
			return fmt.Errorf("%s: expected a non-null value, got null", path)
		}
		return nil
	}

	switch want := expected.(type) {
	case map[string]any:
		got, ok := actual.(map[string]any)
		if !ok {
			return fmt.Errorf("%s: expected an object, got %s", path, describe(actual))
		}
		// Sorted so the first reported failure does not depend on map order.
		keys := make([]string, 0, len(want))
		for k := range want {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			child, present := got[k]
			if !present {
				return fmt.Errorf("%s.%s: missing from the response", path, k)
			}
			if err := matchSubset(want[k], child, path+"."+k); err != nil {
				return err
			}
		}
		return nil

	case []any:
		got, ok := actual.([]any)
		if !ok {
			return fmt.Errorf("%s: expected an array, got %s", path, describe(actual))
		}
		if len(got) != len(want) {
			return fmt.Errorf("%s: expected %d element(s), got %d", path, len(want), len(got))
		}
		for i := range want {
			if err := matchSubset(want[i], got[i], fmt.Sprintf("%s[%d]", path, i)); err != nil {
				return err
			}
		}
		return nil

	default:
		if !reflect.DeepEqual(expected, actual) {
			return fmt.Errorf("%s: expected %s, got %s", path, describe(expected), describe(actual))
		}
		return nil
	}
}

// describe renders a decoded JSON value for a failure message.
func describe(value any) string {
	encoded, err := json.Marshal(value)
	if err != nil {
		return fmt.Sprintf("%v", value)
	}
	return string(encoded)
}

// indentJSON pretty-prints a response body, or returns it unchanged when it is
// not JSON at all.
func indentJSON(raw []byte) string {
	var pretty any
	if err := json.Unmarshal(raw, &pretty); err != nil {
		return string(raw)
	}
	encoded, err := json.MarshalIndent(pretty, "", "  ")
	if err != nil {
		return string(raw)
	}
	return string(encoded)
}
