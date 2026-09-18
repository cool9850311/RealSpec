package minimart.application.usecase;

import minimart.application.dto.ProductItem;
import minimart.application.dto.ProductList;
import minimart.infrastructure.repository.ProductRepository;

/** Reads the public catalogue. */
public class ProductUsecase {

  private final ProductRepository products;

  /** A use case over {@code products}. */
  public ProductUsecase(ProductRepository products) {
    this.products = products;
  }

  /**
   * Returns the active catalogue. An empty catalogue is an empty list, not an error, and {@code
   * items} is always a JSON array — never null.
   */
  public ProductList list() {
    return ProductList.of(
        products.listActive().stream()
            .map(p -> new ProductItem(p.id(), p.name(), p.costPoints(), p.stock()))
            .toList());
  }
}
