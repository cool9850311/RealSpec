package controller

import (
	"net/http"

	"github.com/gin-gonic/gin"

	"minimart/internal/application/dto"
	"minimart/internal/application/usecase"
	"minimart/internal/domain/entity"
)

// OrderController serves POST /orders and GET /orders.
type OrderController struct {
	orders *usecase.OrderUsecase
}

// NewOrderController returns an OrderController.
func NewOrderController(orders *usecase.OrderUsecase) *OrderController {
	return &OrderController{orders: orders}
}

// Create redeems one product for points.
//
// The endpoint is intentionally not idempotent: two requests are two
// redemptions, which is the correct behaviour for a shop.
func (ctl *OrderController) Create(c *gin.Context) {
	userID, err := callerID(c)
	if err != nil {
		respondError(c, err)
		return
	}

	var req dto.CreateOrderRequest
	if err := decodeJSON(c, &req); err != nil {
		respondError(c, err)
		return
	}
	if req.ProductID == nil || *req.ProductID <= 0 {
		respondError(c, entity.ErrInvalidRequest)
		return
	}

	created, err := ctl.orders.Redeem(c.Request.Context(), userID, *req.ProductID)
	if err != nil {
		respondError(c, err)
		return
	}
	c.JSON(http.StatusCreated, created)
}

// ListAll returns every user's orders, newest first.
//
// There is no role check here. The route is wired behind middleware.RequireRole
// and that is the only place the rule lives: an authorisation restated in the
// handler is an authorisation that can drift from the one in front of it, and
// the drift is invisible until someone removes the wrong one.
func (ctl *OrderController) ListAll(c *gin.Context) {
	list, err := ctl.orders.ListAll(c.Request.Context())
	if err != nil {
		respondError(c, err)
		return
	}
	c.JSON(http.StatusOK, list)
}

// List returns the caller's own orders, newest first.
func (ctl *OrderController) List(c *gin.Context) {
	userID, err := callerID(c)
	if err != nil {
		respondError(c, err)
		return
	}
	list, err := ctl.orders.ListMine(c.Request.Context(), userID)
	if err != nil {
		respondError(c, err)
		return
	}
	c.JSON(http.StatusOK, list)
}
