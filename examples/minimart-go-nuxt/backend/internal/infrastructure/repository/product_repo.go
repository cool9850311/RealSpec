package repository

import (
	"context"
	"database/sql"
	"fmt"

	"minimart/internal/domain/entity"
)

// ProductRepository reads the products table.
type ProductRepository struct {
	db *sql.DB
}

// NewProductRepository returns a ProductRepository backed by db.
func NewProductRepository(db *sql.DB) *ProductRepository {
	return &ProductRepository{db: db}
}

// ListActive returns every active product ordered by id ascending.
//
// The active filter is applied in SQL. An inactive product is absent from the
// catalogue and unredeemable, so no client can ever hold the id of one.
func (r *ProductRepository) ListActive(ctx context.Context) ([]entity.Product, error) {
	const q = `SELECT id, name, cost_points, stock FROM products WHERE active ORDER BY id`

	rows, err := r.db.QueryContext(ctx, q)
	if err != nil {
		return nil, fmt.Errorf("list active products: %w", err)
	}
	defer rows.Close()

	products := make([]entity.Product, 0)
	for rows.Next() {
		p := entity.Product{Active: true}
		if err := rows.Scan(&p.ID, &p.Name, &p.CostPoints, &p.Stock); err != nil {
			return nil, fmt.Errorf("scan product: %w", err)
		}
		products = append(products, p)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate products: %w", err)
	}
	return products, nil
}
