// Package controller adapts HTTP to the use cases: it decodes, it validates the
// shape of the request, and it renders the result. No business rule lives here.
package controller

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"

	"github.com/gin-gonic/gin"

	"minimart/internal/application/dto"
	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/middleware"
)

// statusByCode maps every error code of the contract to its status. The table
// is the responses of spec/openapi/openapi.yaml, and it is the only place a
// status is chosen.
var statusByCode = map[entity.CodeError]int{
	entity.ErrInvalidRequest:     http.StatusBadRequest,
	entity.ErrInvalidCredentials: http.StatusUnauthorized,
	entity.ErrUnauthenticated:    http.StatusUnauthorized,
	entity.ErrForbidden:          http.StatusForbidden,
	entity.ErrProductNotFound:    http.StatusNotFound,
	entity.ErrInsufficientPoints: http.StatusUnprocessableEntity,
	entity.ErrOutOfStock:         http.StatusUnprocessableEntity,
}

// respondError renders err as the flat { "error": "<CODE>" } body.
//
// Anything that is not one of the contract's codes is a defect in this service,
// not a message for the caller: it is logged in full and answered with a 500
// that discloses nothing about the failure.
func respondError(c *gin.Context, err error) {
	var code entity.CodeError
	if errors.As(err, &code) {
		if status, ok := statusByCode[code]; ok {
			c.AbortWithStatusJSON(status, dto.ErrorResponse{Error: string(code)})
			return
		}
	}
	log.Printf("unhandled error on %s %s: %v", c.Request.Method, c.Request.URL.Path, err)
	c.AbortWithStatusJSON(http.StatusInternalServerError,
		dto.ErrorResponse{Error: "INTERNAL_ERROR"})
}

// decodeJSON reads the request body into dst.
//
// Unknown fields are refused because every request schema in openapi.yaml
// declares additionalProperties: false; a body with a misspelled key is a
// client bug and is far cheaper to find as a 400 than as a silently ignored
// field.
func decodeJSON(c *gin.Context, dst any) error {
	dec := json.NewDecoder(c.Request.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(dst); err != nil {
		return fmt.Errorf("%w: %v", entity.ErrInvalidRequest, err)
	}
	if dec.More() {
		return fmt.Errorf("%w: unexpected content after the JSON body", entity.ErrInvalidRequest)
	}
	return nil
}

// callerID returns the authenticated caller's id. It can only fail if a route
// was wired without RequireAuth, which is a programming error; reporting it as
// UNAUTHENTICATED keeps the service from serving anybody's data by accident.
func callerID(c *gin.Context) (int, error) {
	id, ok := middleware.UserID(c)
	if !ok {
		return 0, entity.ErrUnauthenticated
	}
	return id, nil
}
