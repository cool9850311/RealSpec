package entity_test

import (
	"errors"
	"testing"

	"minimart/internal/domain/entity"
)

func TestNextStock(t *testing.T) {
	t.Parallel()

	for _, tc := range []struct {
		name    string
		stock   int
		want    int
		wantErr error
	}{
		{"unlimited stays unlimited", -1, -1, nil},
		{"three becomes two", 3, 2, nil},
		{"the last unit becomes zero", 1, 0, nil},
		{"zero is out of stock", 0, 0, entity.ErrOutOfStock},
		{"a corrupt negative is out of stock, not a sale", -2, 0, entity.ErrOutOfStock},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			got, err := entity.NextStock(tc.stock)
			if !errors.Is(err, tc.wantErr) {
				t.Fatalf("NextStock(%d) error = %v, want %v", tc.stock, err, tc.wantErr)
			}
			if err == nil && got != tc.want {
				t.Fatalf("NextStock(%d) = %d, want %d", tc.stock, got, tc.want)
			}
		})
	}
}
