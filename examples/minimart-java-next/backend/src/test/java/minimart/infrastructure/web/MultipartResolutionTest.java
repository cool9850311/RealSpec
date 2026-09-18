package minimart.infrastructure.web;

import static org.assertj.core.api.Assertions.assertThat;

import org.junit.jupiter.api.Test;
import org.springframework.boot.autoconfigure.AutoConfigurations;
import org.springframework.boot.servlet.autoconfigure.MultipartAutoConfiguration;
import org.springframework.boot.test.context.ConfigDataApplicationContextInitializer;
import org.springframework.boot.test.context.runner.WebApplicationContextRunner;
import org.springframework.web.multipart.MultipartResolver;

/**
 * A request body is read as the bytes sent whatever its Content-Type (see {@link JsonBody}), and a
 * multipart/form-data one is the case a controller cannot protect by itself: with a multipart
 * resolver in place, the dispatcher parses such a request into parts before any controller runs,
 * and the servlet container reads the body away doing it. The Go service reads the body as sent, so
 * this service must not resolve multipart requests at all.
 *
 * <p>This is held at the auto-configuration rather than through MockMvc, whose mock request hands
 * the dispatcher its parts without consuming the stream and so passes either way.
 */
class MultipartResolutionTest {

  /** Boot's multipart auto-configuration, fed the service's own application.properties. */
  private final WebApplicationContextRunner runner =
      new WebApplicationContextRunner()
          .withConfiguration(AutoConfigurations.of(MultipartAutoConfiguration.class));

  @Test
  void theServiceConfigurationWiresNoMultipartResolver() {
    runner
        .withInitializer(new ConfigDataApplicationContextInitializer())
        .run(context -> assertThat(context).doesNotHaveBean(MultipartResolver.class));
  }

  /**
   * The control: the same auto-configuration without application.properties does wire one, so the
   * assertion above is about the setting and cannot pass for want of a resolver to find.
   */
  @Test
  void bootWiresOneByDefault() {
    runner.run(context -> assertThat(context).hasSingleBean(MultipartResolver.class));
  }
}
