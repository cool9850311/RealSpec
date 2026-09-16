package controller

import (
	"net/http"
	"time"

	"github.com/gin-gonic/gin"

	"minimart/internal/application/dto"
	"minimart/internal/application/usecase"
	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/middleware"
)

// AuthController serves POST /auth/login and GET /me.
type AuthController struct {
	auth         *usecase.AuthUsecase
	jwtSecret    string
	cookieSecure bool
}

// NewAuthController returns an AuthController.
func NewAuthController(auth *usecase.AuthUsecase, jwtSecret string, cookieSecure bool) *AuthController {
	return &AuthController{auth: auth, jwtSecret: jwtSecret, cookieSecure: cookieSecure}
}

// Login exchanges credentials for the session cookies.
//
// The response body carries the username and role and nothing else: not the
// password hash, and not the token, which exists only as an HttpOnly cookie.
// Alongside it goes the readable session_hint flag, which carries no authority
// and exists so the front end can know it has a session without asking a
// protected endpoint (see middleware.SetSessionCookies).
func (ctl *AuthController) Login(c *gin.Context) {
	var req dto.LoginRequest
	if err := decodeJSON(c, &req); err != nil {
		respondError(c, err)
		return
	}
	if req.Username == "" || req.Password == "" {
		respondError(c, entity.ErrInvalidRequest)
		return
	}

	user, err := ctl.auth.Authenticate(c.Request.Context(), req.Username, req.Password)
	if err != nil {
		respondError(c, err)
		return
	}

	token, err := middleware.IssueToken(ctl.jwtSecret, user, time.Now())
	if err != nil {
		respondError(c, err)
		return
	}
	middleware.SetSessionCookies(c, token, ctl.cookieSecure)

	c.JSON(http.StatusOK, dto.LoginResponse{Username: user.Username, Role: user.Role})
}

// Me returns the caller's own username, role and points balance.
func (ctl *AuthController) Me(c *gin.Context) {
	userID, err := callerID(c)
	if err != nil {
		respondError(c, err)
		return
	}
	me, err := ctl.auth.Me(c.Request.Context(), userID)
	if err != nil {
		respondError(c, err)
		return
	}
	c.JSON(http.StatusOK, me)
}
