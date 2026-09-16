package repository

import (
	"context"
	"database/sql"
	"errors"
	"fmt"

	"minimart/internal/domain/entity"
)

// OrderRepository owns the orders table and the one correctness-critical
// operation minimart has: the redemption transaction.
type OrderRepository struct {
	db *sql.DB
}

// NewOrderRepository returns an OrderRepository backed by db.
func NewOrderRepository(db *sql.DB) *OrderRepository {
	return &OrderRepository{db: db}
}

// ListByUser returns the orders of one user, newest first, joined to the
// product's current name.
//
// The user filter is in SQL. There is no broader query that a handler then
// narrows down, because a handler that forgets to narrow is exactly the bug
// this shape prevents.
func (r *OrderRepository) ListByUser(ctx context.Context, userID int) ([]entity.OrderDetail, error) {
	const q = `
		SELECT o.id, o.user_id, o.product_id, o.cost_points, o.created_at, p.name
		  FROM orders o
		  JOIN products p ON p.id = o.product_id
		 WHERE o.user_id = $1
		 ORDER BY o.id DESC`

	rows, err := r.db.QueryContext(ctx, q, userID)
	if err != nil {
		return nil, fmt.Errorf("list orders: %w", err)
	}
	defer rows.Close()

	orders := make([]entity.OrderDetail, 0)
	for rows.Next() {
		var o entity.OrderDetail
		if err := rows.Scan(&o.ID, &o.UserID, &o.ProductID, &o.CostPoints, &o.CreatedAt, &o.ProductName); err != nil {
			return nil, fmt.Errorf("scan order: %w", err)
		}
		orders = append(orders, o)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate orders: %w", err)
	}
	return orders, nil
}

// ListAll returns every order in the system, newest first, joined to the
// product's current name and to the username of the user who placed it.
//
// This is the one read in minimart that is not scoped to the caller, and it is
// a separate statement from ListByUser rather than the same query with an
// optional WHERE. A filter that a parameter can switch off is a filter that a
// bug can switch off; the authority to run this query is decided in middleware,
// before the handler that calls it is reached.
func (r *OrderRepository) ListAll(ctx context.Context) ([]entity.OrderWithOwner, error) {
	const q = `
		SELECT o.id, o.user_id, o.product_id, o.cost_points, o.created_at, p.name, u.username
		  FROM orders o
		  JOIN products p ON p.id = o.product_id
		  JOIN users    u ON u.id = o.user_id
		 ORDER BY o.id DESC`

	rows, err := r.db.QueryContext(ctx, q)
	if err != nil {
		return nil, fmt.Errorf("list all orders: %w", err)
	}
	defer rows.Close()

	orders := make([]entity.OrderWithOwner, 0)
	for rows.Next() {
		var o entity.OrderWithOwner
		if err := rows.Scan(&o.ID, &o.UserID, &o.ProductID, &o.CostPoints, &o.CreatedAt,
			&o.ProductName, &o.Username); err != nil {
			return nil, fmt.Errorf("scan order: %w", err)
		}
		orders = append(orders, o)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("iterate orders: %w", err)
	}
	return orders, nil
}

// Redeem performs one redemption: deduct points, decrement stock, insert the
// order — all three in a single transaction, or none of them.
//
// This function is the transaction: spec/spec.md points here rather than
// restating it, and every line below is load-bearing:
//
//	BEGIN                                           (READ COMMITTED)
//	SELECT points      ... WHERE id = $1 FOR UPDATE   lock the wallet first
//	SELECT cost, stock ... WHERE id = $2 AND active FOR UPDATE
//	   -- decide, under both locks
//	UPDATE users    SET points = points - cost WHERE id = $1
//	UPDATE products SET stock  = stock  - 1    WHERE id = $2 AND stock > 0
//	INSERT INTO orders ... RETURNING id, created_at
//	COMMIT
//
// Lock order is an invariant, not an accident: the wallet is locked before the
// product, always, so two redemptions cannot take the two locks in opposite
// orders and deadlock. Anything added to this transaction takes its locks in
// the same order or it is wrong, however local the change looks.
//
// Concurrency: two requests for the last unit are serialised by the FOR UPDATE
// on the product row. Under READ COMMITTED the loser's SELECT ... FOR UPDATE
// re-reads the row the winner committed, so it sees stock = 0 and returns
// OUT_OF_STOCK; there is no window in which both read the same stock and both
// write. The decision is never taken on a value read outside the lock.
//
// WHERE stock > 0 on the UPDATE is the second line of defence, and the reason
// the unlimited sentinel survives: for stock = -1 the statement matches no row
// on purpose. The affected-row count is therefore checked against what the
// domain said the next stock must be, and a disagreement aborts the
// transaction rather than committing an order against nothing.
//
// Errors are the domain codes (ErrProductNotFound, ErrInsufficientPoints,
// ErrOutOfStock); each of them rolls back, leaving users.points,
// products.stock and orders all untouched.
func (r *OrderRepository) Redeem(ctx context.Context, userID, productID int) (_ *entity.Order, balanceAfter int, err error) {
	tx, err := r.db.BeginTx(ctx, &sql.TxOptions{Isolation: sql.LevelReadCommitted})
	if err != nil {
		return nil, 0, fmt.Errorf("begin redemption: %w", err)
	}
	defer func() {
		if err != nil {
			// The transaction is already finished on the happy path; rolling
			// back a committed transaction is a no-op error we do not mask the
			// real one with.
			_ = tx.Rollback()
		}
	}()

	// 1. Lock the wallet. Wallet before product, always, so two redemptions can
	//    never take the two locks in opposite orders and deadlock.
	var balance int
	err = tx.QueryRowContext(ctx,
		`SELECT points FROM users WHERE id = $1 FOR UPDATE`, userID).Scan(&balance)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		err = ErrNotFound
		return nil, 0, err
	case err != nil:
		err = fmt.Errorf("lock user %d: %w", userID, err)
		return nil, 0, err
	}

	// 2. Lock the product. No row means "does not exist, or is inactive", which
	//    are the same answer to a caller who is not an admin.
	product := &entity.Product{ID: productID, Active: true}
	err = tx.QueryRowContext(ctx,
		`SELECT cost_points, stock FROM products WHERE id = $1 AND active FOR UPDATE`, productID).
		Scan(&product.CostPoints, &product.Stock)
	switch {
	case errors.Is(err, sql.ErrNoRows):
		product = nil
	case err != nil:
		err = fmt.Errorf("lock product %d: %w", productID, err)
		return nil, 0, err
	}

	// 3+4. Decide, under both locks, in the fixed precedence.
	nextStock, err := entity.EvaluateRedemption(product, balance)
	if err != nil {
		return nil, 0, err
	}

	if _, err = tx.ExecContext(ctx,
		`UPDATE users SET points = points - $2 WHERE id = $1`, userID, product.CostPoints); err != nil {
		err = fmt.Errorf("deduct points: %w", err)
		return nil, 0, err
	}

	res, err := tx.ExecContext(ctx,
		`UPDATE products SET stock = stock - 1 WHERE id = $1 AND stock > 0`, productID)
	if err != nil {
		err = fmt.Errorf("decrement stock: %w", err)
		return nil, 0, err
	}
	affected, err := res.RowsAffected()
	if err != nil {
		err = fmt.Errorf("stock update result: %w", err)
		return nil, 0, err
	}
	// Unlimited stock must match no row; a finite stock must match exactly one.
	wantAffected := int64(0)
	if nextStock != product.Stock {
		wantAffected = 1
	}
	if affected != wantAffected {
		err = fmt.Errorf("stock update affected %d rows, expected %d (stock was %d)",
			affected, wantAffected, product.Stock)
		return nil, 0, err
	}

	order := &entity.Order{UserID: userID, ProductID: productID, CostPoints: product.CostPoints}
	if err = tx.QueryRowContext(ctx,
		`INSERT INTO orders (user_id, product_id, cost_points)
		      VALUES ($1, $2, $3) RETURNING id, created_at`,
		userID, productID, product.CostPoints).Scan(&order.ID, &order.CreatedAt); err != nil {
		err = fmt.Errorf("insert order: %w", err)
		return nil, 0, err
	}

	if err = tx.Commit(); err != nil {
		err = fmt.Errorf("commit redemption: %w", err)
		return nil, 0, err
	}
	return order, balance - product.CostPoints, nil
}
