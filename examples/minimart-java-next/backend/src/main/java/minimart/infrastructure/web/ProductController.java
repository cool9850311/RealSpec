package minimart.infrastructure.web;

import minimart.application.dto.ProductList;
import minimart.application.usecase.ProductUsecase;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

/** Serves GET /products. */
@RestController
@RequestMapping("/api/v1")
public class ProductController {

  private final ProductUsecase products;

  /** A controller over {@code products}. */
  public ProductController(ProductUsecase products) {
    this.products = products;
  }

  /** Returns the active catalogue. It is the one endpoint a visitor can read without a cookie. */
  @GetMapping("/products")
  public ProductList list() {
    return products.list();
  }
}
