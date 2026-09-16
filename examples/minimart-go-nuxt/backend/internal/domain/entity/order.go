package entity

import "time"

// Order is a row of the orders table. CostPoints is the price paid, copied from
// the product at redemption time: changing products.cost_points later must not
// rewrite history.
type Order struct {
	ID         int
	UserID     int
	ProductID  int
	CostPoints int
	CreatedAt  time.Time
}

// OrderDetail is an order joined to its product's current name, which is what
// GET /api/v1/orders renders. The name is read live; the price is not.
type OrderDetail struct {
	Order
	ProductName string
}

// OrderWithOwner is an OrderDetail with the username of the user who placed
// it, which is what GET /api/v1/admin/orders renders. Like the product name it
// is read live: a username can change, the price paid cannot.
type OrderWithOwner struct {
	OrderDetail
	Username string
}

// EvaluateRedemption decides whether the caller may redeem product and, if so,
// what products.stock must become. product is nil when no active product with
// the requested id exists.
//
// The precedence is fixed and is part of the contract:
//
//	PRODUCT_NOT_FOUND → INSUFFICIENT_POINTS → OUT_OF_STOCK
//
// A user who cannot afford anything is told about their points before being
// told about stock, so the message they see does not change as other people
// shop.
func EvaluateRedemption(product *Product, balance int) (nextStock int, err error) {
	if product == nil || !product.Active {
		return 0, ErrProductNotFound
	}
	if balance < product.CostPoints {
		return 0, ErrInsufficientPoints
	}
	return NextStock(product.Stock)
}
