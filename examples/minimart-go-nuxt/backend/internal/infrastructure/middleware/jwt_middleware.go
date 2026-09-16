// Package middleware holds the HTTP concerns that are not any one endpoint's:
// the session cookie and the CORS policy.
package middleware

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strconv"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/golang-jwt/jwt/v5"

	"minimart/internal/application/dto"
	"minimart/internal/domain/entity"
)

const (
	// TokenCookieName is the session itself: the signed JWT, HttpOnly, which
	// script can neither read nor forge.
	TokenCookieName = "token"

	// SessionHintCookieName is a readable flag that says "this browser was
	// given a token cookie". It carries no authority whatsoever — the server
	// never reads it, and forging it buys nothing but a 401 — it exists so the
	// SPA can tell "signed out" from "signed in" WITHOUT probing a protected
	// endpoint. Without it the only way to answer that question is to call
	// GET /api/v1/me and let it fail, and a browser records a failed request as
	// an error-level console entry: an anonymous page load would then be
	// indistinguishable from a broken one, both to a developer reading the
	// console and to the `console has no errors` step of spec/bdd/format.yml.
	SessionHintCookieName = "session_hint"

	// sessionHintValue is the only value the hint ever has. It is a flag, not
	// a claim: nothing about the user is written into a cookie script can read.
	sessionHintValue = "1"
	// TokenTTL is how long an issued token stays valid. There is no refresh
	// token: when it expires the user logs in again.
	TokenTTL = 24 * time.Hour

	// contextUserID and contextRole are where RequireAuth leaves the verified
	// claims for the handlers behind it.
	contextUserID = "minimart.userID"
	contextRole   = "minimart.role"
)

// signingMethod is the one algorithm accepted, stated once so issuing and
// parsing cannot drift apart.
var signingMethod = jwt.SigningMethodHS256

// Claims is the payload of the token cookie: the registered claims (sub, exp)
// plus the two fields the front end needs without a round trip.
type Claims struct {
	Username string `json:"username"`
	Role     string `json:"role"`
	jwt.RegisteredClaims
}

// IssueToken signs a token for user, valid for TokenTTL from issuedAt.
// sub is the user id as a string, as the contract states.
func IssueToken(secret string, user *entity.User, issuedAt time.Time) (string, error) {
	claims := Claims{
		Username: user.Username,
		Role:     user.Role,
		RegisteredClaims: jwt.RegisteredClaims{
			Subject:   strconv.Itoa(user.ID),
			IssuedAt:  jwt.NewNumericDate(issuedAt),
			ExpiresAt: jwt.NewNumericDate(issuedAt.Add(TokenTTL)),
		},
	}
	signed, err := jwt.NewWithClaims(signingMethod, claims).SignedString([]byte(secret))
	if err != nil {
		return "", fmt.Errorf("sign token: %w", err)
	}
	return signed, nil
}

// ParseToken verifies a token and returns its claims.
//
// The accepted algorithm list is pinned to HS256, which is what makes `alg:
// none` and an RS256 token whose "public key" is our HMAC secret rejections
// rather than admissions. An absent or unparsable exp is rejected too, so a
// token can never be eternal by omission.
func ParseToken(secret, raw string) (*Claims, error) {
	var claims Claims
	_, err := jwt.ParseWithClaims(raw, &claims,
		func(*jwt.Token) (any, error) { return []byte(secret), nil },
		jwt.WithValidMethods([]string{signingMethod.Alg()}),
		jwt.WithExpirationRequired(),
	)
	if err != nil {
		return nil, fmt.Errorf("parse token: %w", err)
	}
	if _, err := strconv.Atoi(claims.Subject); err != nil {
		return nil, fmt.Errorf("token subject %q is not a user id", claims.Subject)
	}
	return &claims, nil
}

// SetSessionCookies writes the two cookies a successful login produces. It is
// the single place their policy is decided: SameSite=Lax so they survive a
// top-level navigation but not a cross-site POST, Secure wherever TLS is
// terminated in front of the service, and no Max-Age — the JWT's own exp is the
// lifetime, and a session cookie leaves nothing behind when the browser closes.
//
// The token is HttpOnly; the hint deliberately is not. They are written
// together and cleared together, so the readable flag cannot outlive the
// session it describes by more than one request.
func SetSessionCookies(c *gin.Context, token string, secure bool) {
	http.SetCookie(c.Writer, &http.Cookie{
		Name:     TokenCookieName,
		Value:    token,
		Path:     "/",
		HttpOnly: true,
		Secure:   secure,
		SameSite: http.SameSiteLaxMode,
	})
	http.SetCookie(c.Writer, &http.Cookie{
		Name:     SessionHintCookieName,
		Value:    sessionHintValue,
		Path:     "/",
		HttpOnly: false,
		Secure:   secure,
		SameSite: http.SameSiteLaxMode,
	})
}

// clearSessionHintCookie expires the readable flag. It is sent with every 401
// from RequireAuth, which is the server's only chance to tell a browser that
// the token it is holding is no longer usable: the token cookie itself may be
// expired, deleted or simply invalid, and the hint must not survive it and send
// the SPA back to a protected endpoint on every page load.
func clearSessionHintCookie(c *gin.Context, secure bool) {
	http.SetCookie(c.Writer, &http.Cookie{
		Name:     SessionHintCookieName,
		Value:    "",
		Path:     "/",
		MaxAge:   -1,
		HttpOnly: false,
		Secure:   secure,
		SameSite: http.SameSiteLaxMode,
	})
}

// RequireAuth rejects any request without a valid token cookie with
// 401 UNAUTHENTICATED, and otherwise publishes the verified subject and role to
// the handler.
//
// cookieSecure is carried here only so that the hint cookie a 401 clears is
// written with the same attributes as the one login wrote; a Set-Cookie whose
// Secure flag disagrees does not replace the cookie it is meant to expire.
func RequireAuth(secret string, cookieSecure bool) gin.HandlerFunc {
	return func(c *gin.Context) {
		raw, err := c.Cookie(TokenCookieName)
		if err != nil || raw == "" {
			unauthenticated(c, cookieSecure)
			return
		}
		claims, err := ParseToken(secret, raw)
		if err != nil {
			unauthenticated(c, cookieSecure)
			return
		}
		userID, err := strconv.Atoi(claims.Subject)
		if err != nil {
			unauthenticated(c, cookieSecure)
			return
		}
		c.Set(contextUserID, userID)
		c.Set(contextRole, claims.Role)
		c.Next()
	}
}

func unauthenticated(c *gin.Context, cookieSecure bool) {
	clearSessionHintCookie(c, cookieSecure)
	c.AbortWithStatusJSON(http.StatusUnauthorized,
		dto.ErrorResponse{Error: string(entity.ErrUnauthenticated)})
}

// RoleLookup answers "what role does this user hold right now" from the system
// of record. It returns entity.ErrUnauthenticated when the id names no user.
//
// It is a function rather than a repository or use-case type so that this
// package keeps depending on nothing above it: the router supplies the
// implementation, and the middleware knows only that the answer comes from
// somewhere authoritative.
type RoleLookup func(ctx context.Context, userID int) (string, error)

// RequireRole rejects a caller whose stored role is not `required` with
// 403 FORBIDDEN. It must be wired behind RequireAuth, which is what establishes
// the subject it looks up.
//
// The role is READ, not believed. The token carries a role claim and this
// function never consults it: the claim states what was true when the token was
// issued, and a JWT cannot be withdrawn, so a user demoted an hour ago would
// stay an admin until it expired. The cost is one indexed primary-key read on a
// request that is about to do more work than that anyway.
//
// A subject that no longer exists is 401, not 403: there is nobody to forbid,
// and the caller's remedy is a new session rather than different permissions.
func RequireRole(required string, lookup RoleLookup, cookieSecure bool) gin.HandlerFunc {
	return func(c *gin.Context) {
		userID, ok := UserID(c)
		if !ok {
			// Reachable only by wiring this without RequireAuth in front of
			// it. That is a defect in the route table, and the safe reading of
			// "no subject" is "no session".
			unauthenticated(c, cookieSecure)
			return
		}

		role, err := lookup(c.Request.Context(), userID)
		switch {
		case errors.Is(err, entity.ErrUnauthenticated):
			unauthenticated(c, cookieSecure)
			return
		case err != nil:
			// The caller learns nothing about a failure that is ours; the log
			// keeps the cause.
			log.Printf("role lookup for user %d on %s %s: %v",
				userID, c.Request.Method, c.Request.URL.Path, err)
			c.AbortWithStatusJSON(http.StatusInternalServerError,
				dto.ErrorResponse{Error: "INTERNAL_ERROR"})
			return
		}

		if role != required {
			// Not a 404. The endpoint is published in openapi.yaml, so its
			// existence is not a secret a 404 could keep; the honest answer is
			// that the caller lacks the authority, which is what 403 says.
			// The session hint is left alone on purpose — this session is
			// perfectly valid, and expiring the hint would send the front end
			// back to the login page over a permission it never had.
			c.AbortWithStatusJSON(http.StatusForbidden,
				dto.ErrorResponse{Error: string(entity.ErrForbidden)})
			return
		}
		c.Next()
	}
}

// UserID returns the authenticated caller's id. The second result is false when
// the handler was reached without RequireAuth in front of it, which is a wiring
// bug rather than a request the caller can provoke.
func UserID(c *gin.Context) (int, bool) {
	v, ok := c.Get(contextUserID)
	if !ok {
		return 0, false
	}
	id, ok := v.(int)
	return id, ok
}

// Role returns the authenticated caller's role.
func Role(c *gin.Context) (string, bool) {
	v, ok := c.Get(contextRole)
	if !ok {
		return "", false
	}
	role, ok := v.(string)
	return role, ok
}
