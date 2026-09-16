package entity

// UnlimitedStock is the sentinel stored in products.stock for a product that
// never runs out. It is never decremented and never written by any other value.
const UnlimitedStock = -1

// Product is a row of the products table.
type Product struct {
	ID         int
	Name       string
	CostPoints int
	Stock      int
	Active     bool
}

// NextStock returns the value products.stock must hold after one redemption.
//
//	-1 → -1   (unlimited, the sentinel is never written away)
//	 n → n-1  for n > 0
//	 0 → ErrOutOfStock
//
// Any other non-positive value is treated as out of stock as well: the only
// writer of the column is the redemption transaction, which cannot produce one,
// so reaching that branch means the data is already wrong and the safe answer
// is to refuse rather than to sell.
func NextStock(stock int) (int, error) {
	switch {
	case stock == UnlimitedStock:
		return UnlimitedStock, nil
	case stock > 0:
		return stock - 1, nil
	default:
		return 0, ErrOutOfStock
	}
}
