package minimart.infrastructure.web;

import org.springframework.context.annotation.Configuration;
import org.springframework.http.MediaType;
import org.springframework.web.servlet.config.annotation.ContentNegotiationConfigurer;
import org.springframework.web.servlet.config.annotation.WebMvcConfigurer;

/**
 * Spring MVC settings that make the transport behave like the contract rather than like a content
 * negotiation framework.
 */
@Configuration(proxyBeanMethods = false)
public class WebConfig implements WebMvcConfigurer {

  /**
   * Every response of this API is JSON, whatever the request's Accept header says. Spring would
   * otherwise answer 406 to a client that asked only for, say, text/html — a status the contract
   * does not have, and a request the Go service answers with its JSON like any other.
   */
  @Override
  public void configureContentNegotiation(ContentNegotiationConfigurer negotiation) {
    negotiation.ignoreAcceptHeader(true).defaultContentType(MediaType.APPLICATION_JSON);
  }
}
