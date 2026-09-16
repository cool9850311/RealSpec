package dto

// Product is one entry of the catalogue. Stock is the number of units left,
// or -1 for unlimited.
type Product struct {
	ID         int    `json:"id"`
	Name       string `json:"name"`
	CostPoints int    `json:"cost_points"`
	Stock      int    `json:"stock"`
}

// ProductList is the 200 body of GET /api/v1/products. The list is not
// paginated; Total is present so the shape matches a paginated list if a later
// example needs one.
type ProductList struct {
	Items []Product `json:"items"`
	Total int       `json:"total"`
}
