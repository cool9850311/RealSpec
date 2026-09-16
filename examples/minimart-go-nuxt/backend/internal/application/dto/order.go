package dto

// CreateOrderRequest is the body of POST /api/v1/orders.
//
// ProductID is a pointer so that "absent" and "0" are distinguishable: both are
// 400 INVALID_REQUEST, but only because the handler rejects them, not because
// the zero value silently stood in for a missing field.
type CreateOrderRequest struct {
	ProductID *int `json:"product_id"`
}

// CreateOrderResponse is the 201 body of POST /api/v1/orders.
type CreateOrderResponse struct {
	ID           int `json:"id"`
	CostPoints   int `json:"cost_points"`
	BalanceAfter int `json:"balance_after"`
}

// Order is one entry of the caller's order history. ProductName is the
// product's name as it is now; CostPoints is the price that was actually paid.
type Order struct {
	ID          int    `json:"id"`
	ProductName string `json:"product_name"`
	CostPoints  int    `json:"cost_points"`
	CreatedAt   string `json:"created_at"`
}

// OrderList is the 200 body of GET /api/v1/orders.
type OrderList struct {
	Items []Order `json:"items"`
	Total int     `json:"total"`
}

// AdminOrder is one entry of the all-users order history: an Order plus the
// username of the user who placed it.
//
// It is a type of its own rather than an embedding of Order, because the two
// responses are two contracts: a field added to the admin listing must not be
// able to widen what GET /api/v1/orders hands to its own caller.
type AdminOrder struct {
	ID          int    `json:"id"`
	Username    string `json:"username"`
	ProductName string `json:"product_name"`
	CostPoints  int    `json:"cost_points"`
	CreatedAt   string `json:"created_at"`
}

// AdminOrderList is the 200 body of GET /api/v1/admin/orders.
type AdminOrderList struct {
	Items []AdminOrder `json:"items"`
	Total int          `json:"total"`
}
