package usecase

import (
	"context"
	"errors"
	"time"

	"minimart/internal/application/dto"
	"minimart/internal/domain/entity"
	"minimart/internal/infrastructure/repository"
)

// OrderUsecase redeems products and lists the caller's own orders.
type OrderUsecase struct {
	orders *repository.OrderRepository
}

// NewOrderUsecase returns an OrderUsecase over orders.
func NewOrderUsecase(orders *repository.OrderRepository) *OrderUsecase {
	return &OrderUsecase{orders: orders}
}

// Redeem performs one redemption on behalf of userID.
//
// The three writes and the decision all happen inside OrderRepository.Redeem,
// in one transaction: splitting the read from the write here is exactly the
// race the contract forbids, so there is nothing to orchestrate.
func (uc *OrderUsecase) Redeem(ctx context.Context, userID, productID int) (*dto.CreateOrderResponse, error) {
	order, balanceAfter, err := uc.orders.Redeem(ctx, userID, productID)
	switch {
	case errors.Is(err, repository.ErrNotFound):
		// The token's subject no longer exists.
		return nil, entity.ErrUnauthenticated
	case err != nil:
		return nil, err
	}
	return &dto.CreateOrderResponse{
		ID:           order.ID,
		CostPoints:   order.CostPoints,
		BalanceAfter: balanceAfter,
	}, nil
}

// ListAll returns every user's orders, newest first, each carrying its owner's
// username. The authority to call it is established before the handler runs;
// nothing here re-checks it, because a rule enforced in two places is a rule
// that can disagree with itself.
func (uc *OrderUsecase) ListAll(ctx context.Context) (*dto.AdminOrderList, error) {
	orders, err := uc.orders.ListAll(ctx)
	if err != nil {
		return nil, err
	}

	items := make([]dto.AdminOrder, 0, len(orders))
	for _, o := range orders {
		items = append(items, dto.AdminOrder{
			ID:          o.ID,
			Username:    o.Username,
			ProductName: o.ProductName,
			CostPoints:  o.CostPoints,
			CreatedAt:   o.CreatedAt.UTC().Format(time.RFC3339Nano),
		})
	}
	return &dto.AdminOrderList{Items: items, Total: len(items)}, nil
}

// ListMine returns the caller's own orders, newest first.
func (uc *OrderUsecase) ListMine(ctx context.Context, userID int) (*dto.OrderList, error) {
	orders, err := uc.orders.ListByUser(ctx, userID)
	if err != nil {
		return nil, err
	}

	items := make([]dto.Order, 0, len(orders))
	for _, o := range orders {
		items = append(items, dto.Order{
			ID:          o.ID,
			ProductName: o.ProductName,
			CostPoints:  o.CostPoints,
			// Rendered in UTC so the wire format is the one the contract shows
			// regardless of the database session's time zone.
			CreatedAt: o.CreatedAt.UTC().Format(time.RFC3339Nano),
		})
	}
	return &dto.OrderList{Items: items, Total: len(items)}, nil
}
