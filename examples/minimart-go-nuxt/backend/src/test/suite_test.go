package test

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"

	"github.com/cucumber/godog"
)

// TestMain builds the service image once for the whole suite. Per-scenario
// containers are then a start, not a build, which is what makes one stack per
// scenario affordable.
func TestMain(m *testing.M) {
	if err := buildServiceImage(context.Background()); err != nil {
		fmt.Fprintf(os.Stderr, "building %s failed: %v\n", serviceImage, err)
		os.Exit(1)
	}
	os.Exit(m.Run())
}

// buildServiceImage builds backend/Dockerfile exactly the way
// local/docker-compose.yml does: the context is examples/minimart-go-nuxt and
// the Dockerfile is backend/Dockerfile, so the image under test and the image a
// human runs locally are built from the same two inputs.
func buildServiceImage(ctx context.Context) error {
	root, err := filepath.Abs(buildContextDir)
	if err != nil {
		return fmt.Errorf("resolve build context: %w", err)
	}
	cmd := exec.CommandContext(ctx, "docker", "build",
		"-f", filepath.Join(root, "backend", "Dockerfile"),
		"-t", serviceImage,
		root,
	)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

// TestAPIFeatures runs spec/bdd/api/*.feature.
func TestAPIFeatures(t *testing.T) {
	paths := []string{"../../../spec/bdd/api"}
	if p := os.Getenv("GODOG_PATHS"); p != "" {
		paths = []string{p}
	}

	concurrency := envInt("GODOG_CONCURRENCY", defaultConcurrency())
	t.Logf("running scenarios at concurrency %d (GOMAXPROCS=%d)", concurrency, runtime.GOMAXPROCS(0))

	suite := godog.TestSuite{
		Name:                "minimart-api",
		ScenarioInitializer: InitializeScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    paths,
			Tags:     os.Getenv("GODOG_TAGS"),
			TestingT: t,
			// Strict: an undefined or pending step fails the suite. A step the
			// registry declares and nobody implemented must not pass as a
			// yellow line in the output.
			Strict:        true,
			StopOnFailure: envBool("GODOG_STOP_ON_FAILURE", false),
			Concurrency:   concurrency,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("API feature suite failed")
	}
}

// InitializeScenario binds one step definition to every API step id in
// spec/bdd/format.yml. The regular expressions are the registry's `pattern`
// fields copied verbatim: the registry is the grammar, and a runner that
// paraphrased it would be a second, disagreeing grammar.
//
// The e2e step ids of the same registry are implemented by the Playwright
// runner in frontend/tests/e2e/steps; the three infrastructure steps are shared
// by both surfaces and are implemented here for this one.
func InitializeScenario(sc *godog.ScenarioContext) {
	s := &ScenarioCtx{}

	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		return ctx, s.startInfra(ctx)
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		s.stopInfra(ctx)
		return ctx, nil
	})

	// ── Shared: infrastructure setup ────────────────────────────────────────
	sc.Step(`^run migration$`, s.runMigration)
	sc.Step(`^in PostgreSQL:$`, s.execPostgres)

	// ── API: HTTP request ───────────────────────────────────────────────────
	sc.Step(`^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$`, s.httpRequest)
	sc.Step(`^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+) is called concurrently:$`, s.httpRequestConcurrent)

	// ── API: response assertions ────────────────────────────────────────────
	sc.Step(`^response status is ([1-5][0-9]{2})$`, s.responseStatusIs)
	sc.Step(`^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$`, s.responseSetStatusCount)
	sc.Step(`^response body contains:$`, s.responseBodyContains)
	sc.Step(`^response body does not contain "([^"]+)"$`, s.responseBodyDoesNotContain)
	sc.Step(`^response header "([^"]+)" contains "([^"]+)"$`, s.responseHeaderContains)
	sc.Step(`^save response body field "([a-zA-Z_][a-zA-Z0-9_]*)" as "([a-z][a-zA-Z0-9]+)"$`, s.saveResponseBodyField)
	sc.Step(`^save response cookie "([a-zA-Z0-9_-]+)" as "([a-z][a-zA-Z0-9]+)"$`, s.saveResponseCookie)

	// ── Shared: infrastructure query assertions ─────────────────────────────
	sc.Step(`^in PostgreSQL query returns ([0-9]+) rows?:$`, s.postgresQueryReturns)
}

// One unit is a stack of containers, not a goroutine, so: half the CPUs.
// GOMAXPROCS rather than NumCPU — it is the count an operator can lower.
func defaultConcurrency() int {
	return max(1, runtime.GOMAXPROCS(0)/2)
}

func envBool(key string, fallback bool) bool {
	raw := os.Getenv(key)
	if raw == "" {
		return fallback
	}
	v, err := strconv.ParseBool(raw)
	if err != nil {
		return fallback
	}
	return v
}

func envInt(key string, fallback int) int {
	raw := os.Getenv(key)
	if raw == "" {
		return fallback
	}
	v, err := strconv.Atoi(raw)
	if err != nil || v < 1 {
		return fallback
	}
	return v
}
