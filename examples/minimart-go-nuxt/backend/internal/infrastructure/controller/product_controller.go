package controller

import (
	"net/http"

	"github.com/gin-gonic/gin"

	"minimart/internal/application/usecase"
)

// ProductController serves GET /products.
type ProductController struct {
	products *usecase.ProductUsecase
}

// NewProductController returns a ProductController.
func NewProductController(products *usecase.ProductUsecase) *ProductController {
	return &ProductController{products: products}
}

// List returns the active catalogue. It is the one endpoint a visitor can read
// without a cookie.
func (ctl *ProductController) List(c *gin.Context) {
	list, err := ctl.products.List(c.Request.Context())
	if err != nil {
		respondError(c, err)
		return
	}
	c.JSON(http.StatusOK, list)
}
