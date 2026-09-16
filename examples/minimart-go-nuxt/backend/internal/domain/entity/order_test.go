package entity_test

import (
	"errors"
	"testing"

	"minimart/internal/domain/entity"
)

// TestEvaluateRedemptionPrecedence pins the order the contract fixes:
// PRODUCT_NOT_FOUND → INSUFFICIENT_POINTS → OUT_OF_STOCK. Each case below is
// one where more than one rule applies, so only the precedence distinguishes
// the answers.
func TestEvaluateRedemptionPrecedence(t *testing.T) {
	t.Parallel()

	for _, tc := range []struct {
		name    string
		product *entity.Product
		balance int
		want    error
	}{
		{
			name:    "no such product",
			product: nil,
			balance: 1000,
			want:    entity.ErrProductNotFound,
		},
		{
			name:    "inactive and unaffordable is not found, not insufficient",
			product: &entity.Product{CostPoints: 50, Stock: 5, Active: false},
			balance: 0,
			want:    entity.ErrProductNotFound,
		},
		{
			name:    "affordable but sold out",
			product: &entity.Product{CostPoints: 50, Stock: 0, Active: true},
			balance: 100,
			want:    entity.ErrOutOfStock,
		},
		{
			name:    "unaffordable and sold out reports the points first",
			product: &entity.Product{CostPoints: 50, Stock: 0, Active: true},
			balance: 10,
			want:    entity.ErrInsufficientPoints,
		},
		{
			name:    "exactly affordable is allowed",
			product: &entity.Product{CostPoints: 50, Stock: 1, Active: true},
			balance: 50,
			want:    nil,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			_, err := entity.EvaluateRedemption(tc.product, tc.balance)
			if !errors.Is(err, tc.want) {
				t.Fatalf("EvaluateRedemption = %v, want %v", err, tc.want)
			}
		})
	}
}

func TestEvaluateRedemptionReturnsNextStock(t *testing.T) {
	t.Parallel()

	unlimited := &entity.Product{CostPoints: 20, Stock: entity.UnlimitedStock, Active: true}
	next, err := entity.EvaluateRedemption(unlimited, 100)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if next != entity.UnlimitedStock {
		t.Fatalf("unlimited stock became %d, want %d", next, entity.UnlimitedStock)
	}
}
