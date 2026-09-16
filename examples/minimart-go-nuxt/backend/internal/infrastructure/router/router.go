// Package router wires the whole service: repositories over the database, use
// cases over the repositories, controllers over the use cases, and the routes
// of spec/openapi/openapi.yaml over the controllers.
package router

import (
	"database/sql"

	"github.com/gin-gonic/gin"

	"minimart/internal/application/usecase"
	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/config"
	"minimart/internal/infrastructure/controller"
	"minimart/internal/infrastructure/middleware"
	"minimart/internal/infrastructure/repository"
)

// New builds the HTTP handler. Every route of the contract is registered here
// and nowhere else, so the list below can be read against openapi.yaml line by
// line.
//
// Routes not listed keep gin's own 404. The contract's error codes are a closed
// set describing this API's failures, and "that path is not part of this API"
// is not one of them; inventing a code for it would be inventing contract.
func New(cfg *config.Config, db *sql.DB) *gin.Engine {
	authUC := usecase.NewAuthUsecase(repository.NewUserRepository(db))
	productUC := usecase.NewProductUsecase(repository.NewProductRepository(db))
	orderUC := usecase.NewOrderUsecase(repository.NewOrderRepository(db))

	authCtl := controller.NewAuthController(authUC, cfg.JWTSecret, cfg.CookieSecure)
	productCtl := controller.NewProductController(productUC)
	orderCtl := controller.NewOrderController(orderUC)

	r := gin.New()
	r.Use(gin.Logger(), gin.Recovery())
	// The service is reached through the reverse proxy; no client-supplied
	// forwarding header is trusted to name the caller.
	if err := r.SetTrustedProxies(nil); err != nil {
		panic(err)
	}
	r.Use(middleware.CORS(cfg.FrontendOrigin))

	v1 := r.Group("/api/v1")
	{
		v1.POST("/auth/login", authCtl.Login)
		v1.GET("/products", productCtl.List)

		requireAuth := middleware.RequireAuth(cfg.JWTSecret, cfg.CookieSecure)

		authed := v1.Group("", requireAuth)
		{
			authed.GET("/me", authCtl.Me)
			authed.POST("/orders", orderCtl.Create)
			authed.GET("/orders", orderCtl.List)
		}

		// Authentication, then authorisation, both in middleware and in that
		// order. RequireRole is given the use case's lookup rather than the
		// token's role claim, which is what makes the database the authority
		// on what a caller may do.
		admin := v1.Group("/admin", requireAuth,
			middleware.RequireRole(entity.RoleAdmin, authUC.RoleOf, cfg.CookieSecure))
		{
			admin.GET("/orders", orderCtl.ListAll)
		}
	}
	return r
}
