package usecase

import (
	"context"

	"minimart/internal/application/dto"
	"minimart/internal/infrastructure/repository"
)

// ProductUsecase reads the public catalogue.
type ProductUsecase struct {
	products *repository.ProductRepository
}

// NewProductUsecase returns a ProductUsecase over products.
func NewProductUsecase(products *repository.ProductRepository) *ProductUsecase {
	return &ProductUsecase{products: products}
}

// List returns the active catalogue. An empty catalogue is an empty list, not
// an error, and Items is always a JSON array — never null.
func (uc *ProductUsecase) List(ctx context.Context) (*dto.ProductList, error) {
	products, err := uc.products.ListActive(ctx)
	if err != nil {
		return nil, err
	}

	items := make([]dto.Product, 0, len(products))
	for _, p := range products {
		items = append(items, dto.Product{
			ID:         p.ID,
			Name:       p.Name,
			CostPoints: p.CostPoints,
			Stock:      p.Stock,
		})
	}
	return &dto.ProductList{Items: items, Total: len(items)}, nil
}
