package middleware

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// CORS answers cross-origin requests according to FRONTEND_ORIGIN.
//
//	""  — send no CORS headers. Correct for the deployments of this example,
//	      where the reverse proxy puts the front end and the API on one origin
//	      and there is no cross-origin request to allow.
//	"*" — reflect whatever Origin asked. Credentials cannot be combined with a
//	      literal "*", so the wildcard is implemented by echoing the origin.
//	host — allow exactly that origin and no other.
//
// Allow-Credentials is always true when an origin is allowed at all: the
// session is a cookie, so a cross-origin front end that cannot send credentials
// cannot log in.
func CORS(allowedOrigin string) gin.HandlerFunc {
	return func(c *gin.Context) {
		if allowedOrigin != "" {
			// The response depends on the request's Origin, so it is not
			// cacheable without it — whether or not we ended up allowing it.
			c.Writer.Header().Add("Vary", "Origin")

			origin := c.GetHeader("Origin")
			if origin != "" && (allowedOrigin == "*" || allowedOrigin == origin) {
				h := c.Writer.Header()
				h.Set("Access-Control-Allow-Origin", origin)
				h.Set("Access-Control-Allow-Credentials", "true")
				h.Set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
				h.Set("Access-Control-Allow-Headers", "Content-Type")
				h.Set("Access-Control-Max-Age", "600")
			}
		}

		if c.Request.Method == http.MethodOptions {
			// A preflight carries no body and reaches no handler. Whether the
			// browser then proceeds is decided by the headers set above.
			c.AbortWithStatus(http.StatusNoContent)
			return
		}
		c.Next()
	}
}
