package minimart.infrastructure.config;

import com.zaxxer.hikari.HikariConfig;
import com.zaxxer.hikari.HikariDataSource;
import java.time.Duration;
import javax.sql.DataSource;
import minimart.application.usecase.AuthUsecase;
import minimart.application.usecase.OrderUsecase;
import minimart.application.usecase.ProductUsecase;
import minimart.infrastructure.repository.OrderRepository;
import minimart.infrastructure.repository.ProductRepository;
import minimart.infrastructure.repository.SchemaMigrator;
import minimart.infrastructure.repository.UserRepository;
import minimart.infrastructure.security.RoleLookup;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.boot.web.server.ConfigurableWebServerFactory;
import org.springframework.boot.web.server.WebServerFactoryCustomizer;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.jdbc.support.JdbcTransactionManager;
import org.springframework.transaction.PlatformTransactionManager;

/**
 * Wires the whole service: the pool over the database, repositories over the pool, use cases over
 * the repositories. Controllers and the security chain are built over these beans; the routes of
 * spec/openapi/openapi.yaml are in the controllers and in SecurityConfig.
 *
 * <p>Everything here is built from {@link AppConfig}, never from Spring properties, so the six
 * variables AppConfig documents are the whole configuration surface.
 */
@Configuration(proxyBeanMethods = false)
public class ApplicationWiring {

  private static final Logger log = LoggerFactory.getLogger(ApplicationWiring.class);

  /**
   * How long boot waits for PostgreSQL to accept a connection before giving up. Container start
   * order is not guaranteed, so "not up yet" is an expected state for the first few hundred
   * milliseconds; a database that is still unreachable after this is a failed deployment.
   */
  static final Duration DB_WAIT_TIMEOUT = Duration.ofSeconds(60);

  /**
   * The connection pool, opened and proven before anything else starts, so that the HTTP port opens
   * only once the service can actually serve. When SCHEMA_AUTO_MIGRATE is true the schema is
   * migrated here too, for the same reason: no request can be accepted against a schema that is
   * still being built.
   */
  @Bean(destroyMethod = "close")
  public HikariDataSource dataSource(AppConfig config) {
    HikariConfig hikari = new HikariConfig();
    hikari.setPoolName("minimart");
    hikari.setJdbcUrl(config.jdbcUrl());
    hikari.setUsername(config.dbUser());
    hikari.setPassword(config.dbPassword());
    // The same limits as the Go service's pool: 20 open, 10 kept idle, recycled hourly.
    hikari.setMaximumPoolSize(20);
    hikari.setMinimumIdle(10);
    hikari.setMaxLifetime(Duration.ofHours(1).toMillis());
    // Hikari retries its first connection for this long and then fails the constructor — the
    // "wait for the database, then give up loudly" loop, without writing one.
    hikari.setInitializationFailTimeout(DB_WAIT_TIMEOUT.toMillis());

    HikariDataSource dataSource = new HikariDataSource(hikari);
    if (config.schemaAutoMigrate()) {
      try {
        SchemaMigrator.migrate(dataSource);
      } catch (RuntimeException e) {
        dataSource.close();
        throw e;
      }
      log.info("schema migrated from {}", SchemaMigrator.LOCATION);
    }
    return dataSource;
  }

  /** The transaction manager the redemption runs on. */
  @Bean
  public PlatformTransactionManager transactionManager(DataSource dataSource) {
    return new JdbcTransactionManager(dataSource);
  }

  /** The one SQL entry point the repositories share. */
  @Bean
  public JdbcClient jdbcClient(DataSource dataSource) {
    return JdbcClient.create(dataSource);
  }

  @Bean
  UserRepository userRepository(JdbcClient jdbc) {
    return new UserRepository(jdbc);
  }

  @Bean
  ProductRepository productRepository(JdbcClient jdbc) {
    return new ProductRepository(jdbc);
  }

  @Bean
  OrderRepository orderRepository(JdbcClient jdbc, PlatformTransactionManager transactions) {
    return new OrderRepository(jdbc, transactions);
  }

  @Bean
  AuthUsecase authUsecase(UserRepository users) {
    return new AuthUsecase(users);
  }

  @Bean
  ProductUsecase productUsecase(ProductRepository products) {
    return new ProductUsecase(products);
  }

  @Bean
  OrderUsecase orderUsecase(OrderRepository orders) {
    return new OrderUsecase(orders);
  }

  /**
   * The role check's source of truth is the use case's database read — not the token's role claim,
   * which is what makes the database the authority on what a caller may do.
   */
  @Bean
  RoleLookup roleLookup(AuthUsecase auth) {
    return auth::roleOf;
  }

  /**
   * PORT, applied to the embedded server. It runs after Spring Boot's own customizer (this one is
   * unordered, i.e. last), so nothing in Spring's property sources can override it.
   */
  @Bean
  WebServerFactoryCustomizer<ConfigurableWebServerFactory> serverPort(AppConfig config) {
    return factory -> factory.setPort(config.port());
  }
}
