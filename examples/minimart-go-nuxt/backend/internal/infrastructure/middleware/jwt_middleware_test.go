package middleware_test

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/gin-gonic/gin"

	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/middleware"
)

const testSecret = "unit-test-secret"

func TestIssueAndParseTokenRoundTrip(t *testing.T) {
	t.Parallel()

	user := &entity.User{ID: 42, Username: "alice", Role: entity.RoleGuest}
	issuedAt := time.Now()

	token, err := middleware.IssueToken(testSecret, user, issuedAt)
	if err != nil {
		t.Fatalf("IssueToken: %v", err)
	}

	claims, err := middleware.ParseToken(testSecret, token)
	if err != nil {
		t.Fatalf("ParseToken: %v", err)
	}
	if claims.Subject != "42" {
		t.Errorf("sub = %q, want %q", claims.Subject, "42")
	}
	if claims.Username != "alice" {
		t.Errorf("username = %q, want %q", claims.Username, "alice")
	}
	if claims.Role != entity.RoleGuest {
		t.Errorf("role = %q, want %q", claims.Role, entity.RoleGuest)
	}
	if claims.ExpiresAt == nil || claims.IssuedAt == nil {
		t.Fatal("token carries no exp or no iat")
	}
	// Both claims are whole seconds on the wire, so they are compared to each
	// other rather than to issuedAt, whose nanoseconds do not survive encoding.
	if got := claims.ExpiresAt.Sub(claims.IssuedAt.Time); got != middleware.TokenTTL {
		t.Errorf("exp is %s after iat, want %s", got, middleware.TokenTTL)
	}
	if got := claims.IssuedAt.Sub(issuedAt); got < -time.Second || got > 0 {
		t.Errorf("iat is %s from the requested issue time, want within one second before it", got)
	}
}

func TestParseTokenRejectsExpired(t *testing.T) {
	t.Parallel()

	user := &entity.User{ID: 1, Username: "alice", Role: entity.RoleGuest}
	// Issued far enough in the past that it expired a minute ago.
	token, err := middleware.IssueToken(testSecret, user, time.Now().Add(-middleware.TokenTTL-time.Minute))
	if err != nil {
		t.Fatalf("IssueToken: %v", err)
	}
	if _, err := middleware.ParseToken(testSecret, token); err == nil {
		t.Fatal("an expired token was accepted")
	}
}

func TestParseTokenRejectsAnotherSecret(t *testing.T) {
	t.Parallel()

	user := &entity.User{ID: 1, Username: "alice", Role: entity.RoleGuest}
	token, err := middleware.IssueToken("someone else's secret", user, time.Now())
	if err != nil {
		t.Fatalf("IssueToken: %v", err)
	}
	if _, err := middleware.ParseToken(testSecret, token); err == nil {
		t.Fatal("a token signed with another secret was accepted")
	}
}

// TestParseTokenRejectsAlgNone forges the classic unsigned token: the same
// claims, an "alg": "none" header and an empty signature. Accepting it would
// let anybody mint any session, so it is asserted rather than assumed.
func TestParseTokenRejectsAlgNone(t *testing.T) {
	t.Parallel()

	header := segment(t, map[string]any{"alg": "none", "typ": "JWT"})
	payload := segment(t, map[string]any{
		"sub":      "1",
		"username": "alice",
		"role":     entity.RoleAdmin,
		"exp":      time.Now().Add(time.Hour).Unix(),
	})
	forged := strings.Join([]string{header, payload, ""}, ".")

	if _, err := middleware.ParseToken(testSecret, forged); err == nil {
		t.Fatal("an alg:none token was accepted")
	}
}

func TestParseTokenRejectsNonNumericSubject(t *testing.T) {
	t.Parallel()

	// A well-signed token whose subject is not a user id must not reach a
	// handler that would then query with it.
	user := &entity.User{ID: 7, Username: "alice", Role: entity.RoleGuest}
	token, err := middleware.IssueToken(testSecret, user, time.Now())
	if err != nil {
		t.Fatalf("IssueToken: %v", err)
	}
	if _, err := middleware.ParseToken(testSecret, token+"tampered"); err == nil {
		t.Fatal("a tampered token was accepted")
	}
}

func segment(t *testing.T, value map[string]any) string {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("marshal token segment: %v", err)
	}
	return base64.RawURLEncoding.EncodeToString(encoded)
}

// TestSetSessionCookiesWritesBothCookies pins the property the front end depends
// on: the token is HttpOnly and the session hint is not. If the hint ever became
// HttpOnly the SPA could not read it, would fall back to probing /me on every
// anonymous page load, and the browser would record that 401 as a console error.
func TestSetSessionCookiesWritesBothCookies(t *testing.T) {
	t.Parallel()

	for _, secure := range []bool{false, true} {
		recorder := httptest.NewRecorder()
		c, _ := gin.CreateTestContext(recorder)

		middleware.SetSessionCookies(c, "a.b.c", secure)

		cookies := (&http.Response{Header: recorder.Header()}).Cookies()
		byName := map[string]*http.Cookie{}
		for _, cookie := range cookies {
			byName[cookie.Name] = cookie
		}

		token, ok := byName[middleware.TokenCookieName]
		if !ok {
			t.Fatalf("secure=%v: no %q cookie was set", secure, middleware.TokenCookieName)
		}
		if token.Value != "a.b.c" {
			t.Errorf("secure=%v: token value = %q, want the signed token", secure, token.Value)
		}
		if !token.HttpOnly {
			t.Errorf("secure=%v: the token cookie must be HttpOnly", secure)
		}

		hint, ok := byName[middleware.SessionHintCookieName]
		if !ok {
			t.Fatalf("secure=%v: no %q cookie was set", secure, middleware.SessionHintCookieName)
		}
		if hint.HttpOnly {
			t.Errorf("secure=%v: the session hint must be readable by script", secure)
		}
		if hint.Value == "" {
			t.Errorf("secure=%v: the session hint was set to an empty value", secure)
		}
		for _, cookie := range []*http.Cookie{token, hint} {
			if cookie.Path != "/" {
				t.Errorf("secure=%v: %s Path = %q, want /", secure, cookie.Name, cookie.Path)
			}
			if cookie.Secure != secure {
				t.Errorf("secure=%v: %s Secure = %v", secure, cookie.Name, cookie.Secure)
			}
			if cookie.SameSite != http.SameSiteLaxMode {
				t.Errorf("secure=%v: %s SameSite = %v, want Lax", secure, cookie.Name, cookie.SameSite)
			}
		}
	}
}

// TestRequireAuthClearsSessionHintOn401 is the other half of the contract: a
// stale hint must not survive a 401, or the SPA would probe a protected endpoint
// on every page load for the rest of the browser session.
func TestRequireAuthClearsSessionHintOn401(t *testing.T) {
	t.Parallel()

	gin.SetMode(gin.TestMode)
	engine := gin.New()
	engine.GET("/me", middleware.RequireAuth(testSecret, true), func(c *gin.Context) {
		c.Status(http.StatusOK)
	})

	request := httptest.NewRequest(http.MethodGet, "/me", nil)
	request.AddCookie(&http.Cookie{Name: middleware.SessionHintCookieName, Value: "1"})
	recorder := httptest.NewRecorder()
	engine.ServeHTTP(recorder, request)

	if recorder.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want 401", recorder.Code)
	}
	for _, cookie := range (&http.Response{Header: recorder.Header()}).Cookies() {
		if cookie.Name != middleware.SessionHintCookieName {
			continue
		}
		if cookie.MaxAge >= 0 || cookie.Value != "" {
			t.Fatalf("the 401 did not expire the session hint: %+v", cookie)
		}
		return
	}
	t.Fatal("the 401 did not clear the session hint cookie at all")
}

// TestRequireRoleReadsTheStoredRoleNotTheClaim is the unit half of the
// privilege-escalation scenario in spec/bdd/api/auth.feature.
//
// Every case below presents a token that is beyond reproach — correctly signed,
// unexpired, and claiming `admin`. What separates them is only what the lookup
// says, which is what the assertion is for: if RequireRole ever read
// claims.Role instead of asking, all four would pass the guard.
func TestRequireRoleReadsTheStoredRoleNotTheClaim(t *testing.T) {
	t.Parallel()

	lookupFailed := errors.New("the database is down")

	for _, tc := range []struct {
		name       string
		storedRole string
		lookupErr  error
		wantStatus int
		wantBody   string
		wantServed bool
	}{
		{
			name:       "the stored role matches",
			storedRole: entity.RoleAdmin,
			wantStatus: http.StatusOK,
			wantBody:   `{"served":true}`,
			wantServed: true,
		},
		{
			name:       "the stored role is not the required one",
			storedRole: entity.RoleGuest,
			wantStatus: http.StatusForbidden,
			wantBody:   `{"error":"FORBIDDEN"}`,
		},
		{
			name:       "the subject names no user",
			lookupErr:  entity.ErrUnauthenticated,
			wantStatus: http.StatusUnauthorized,
			wantBody:   `{"error":"UNAUTHENTICATED"}`,
		},
		{
			name:       "the lookup itself fails",
			lookupErr:  lookupFailed,
			wantStatus: http.StatusInternalServerError,
			wantBody:   `{"error":"INTERNAL_ERROR"}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()

			// A token whose claim says admin in every case, so the claim
			// cannot be what any of these outcomes is explained by.
			user := &entity.User{ID: 3, Username: "alice", Role: entity.RoleAdmin}
			token, err := middleware.IssueToken(testSecret, user, time.Now())
			if err != nil {
				t.Fatalf("IssueToken: %v", err)
			}

			var lookedUp int
			lookup := func(_ context.Context, userID int) (string, error) {
				lookedUp = userID
				return tc.storedRole, tc.lookupErr
			}

			served := false
			gin.SetMode(gin.TestMode)
			engine := gin.New()
			engine.GET("/admin/orders",
				middleware.RequireAuth(testSecret, false),
				middleware.RequireRole(entity.RoleAdmin, lookup, false),
				func(c *gin.Context) {
					served = true
					c.JSON(http.StatusOK, gin.H{"served": true})
				})

			request := httptest.NewRequest(http.MethodGet, "/admin/orders", nil)
			request.AddCookie(&http.Cookie{Name: middleware.TokenCookieName, Value: token})
			recorder := httptest.NewRecorder()
			engine.ServeHTTP(recorder, request)

			if recorder.Code != tc.wantStatus {
				t.Errorf("status = %d, want %d (body %s)", recorder.Code, tc.wantStatus, recorder.Body)
			}
			if got := strings.TrimSpace(recorder.Body.String()); got != tc.wantBody {
				t.Errorf("body = %s, want %s", got, tc.wantBody)
			}
			if served != tc.wantServed {
				t.Errorf("handler ran = %v, want %v", served, tc.wantServed)
			}
			if lookedUp != user.ID {
				t.Errorf("the role was looked up for user %d, want %d", lookedUp, user.ID)
			}
		})
	}
}

// TestRequireRoleLeavesTheSessionHintAlone: a 403 is not a broken session. If it
// expired the hint the SPA would conclude it had been signed out and send the
// user to the login page over a permission they were never going to have.
func TestRequireRoleLeavesTheSessionHintAlone(t *testing.T) {
	t.Parallel()

	user := &entity.User{ID: 5, Username: "alice", Role: entity.RoleGuest}
	token, err := middleware.IssueToken(testSecret, user, time.Now())
	if err != nil {
		t.Fatalf("IssueToken: %v", err)
	}

	gin.SetMode(gin.TestMode)
	engine := gin.New()
	engine.GET("/admin/orders",
		middleware.RequireAuth(testSecret, true),
		middleware.RequireRole(entity.RoleAdmin,
			func(context.Context, int) (string, error) { return entity.RoleGuest, nil }, true),
		func(c *gin.Context) { c.Status(http.StatusOK) })

	request := httptest.NewRequest(http.MethodGet, "/admin/orders", nil)
	request.AddCookie(&http.Cookie{Name: middleware.TokenCookieName, Value: token})
	request.AddCookie(&http.Cookie{Name: middleware.SessionHintCookieName, Value: "1"})
	recorder := httptest.NewRecorder()
	engine.ServeHTTP(recorder, request)

	if recorder.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403", recorder.Code)
	}
	for _, cookie := range (&http.Response{Header: recorder.Header()}).Cookies() {
		if cookie.Name == middleware.SessionHintCookieName {
			t.Fatalf("the 403 touched the session hint: %+v", cookie)
		}
	}
}
