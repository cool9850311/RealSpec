package minimart.domain;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.junit.jupiter.params.provider.ValueSource;

class StockTest {

  @ParameterizedTest(name = "{0} becomes {1}")
  @CsvSource({
    // unlimited stays unlimited
    "-1, -1",
    // three becomes two
    "3, 2",
    // the last unit becomes zero
    "1, 0",
  })
  void sellsWhatIsThere(int stock, int next) {
    assertThat(Stock.next(stock)).isEqualTo(next);
  }

  @ParameterizedTest(name = "{0} is out of stock")
  // Zero is out of stock; a corrupt negative is out of stock too, not a sale.
  @ValueSource(ints = {0, -2})
  void refusesWhatIsNot(int stock) {
    assertThatThrownBy(() -> Stock.next(stock))
        .isInstanceOfSatisfying(
            DomainException.class, e -> assertThat(e.code()).isEqualTo(ErrorCode.OUT_OF_STOCK));
  }
}
