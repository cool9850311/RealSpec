package minimart;

import minimart.infrastructure.config.AppConfig;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.core.env.StandardEnvironment;
import org.springframework.web.context.support.StandardServletEnvironment;

/**
 * The backend of the RealSpec reference application: login, product list, points redemption.
 *
 * <p>Boot order, and why:
 *
 * <ol>
 *   <li>The environment is read and validated by {@link AppConfig} before Spring starts. A bad
 *       variable is one line naming it and exit status 1 — not a stack trace from deep inside a
 *       bean factory.
 *   <li>Spring starts WITHOUT the operating-system environment as a property source. AppConfig is
 *       the only reader of the environment; removing the source is what makes that true of Spring
 *       too, so a {@code SERVER_PORT} or {@code SPRING_*} variable left in a deployment cannot
 *       quietly override what AppConfig decided.
 *   <li>The validated AppConfig is registered as a bean before any other bean is created, and
 *       everything else is built from it (see ApplicationWiring and SecurityConfig).
 *   <li>The pool connects (waiting up to 60 seconds for PostgreSQL) and, if asked, migrates; only
 *       then does the HTTP port open.
 * </ol>
 *
 * Shutdown is graceful: on SIGTERM in-flight requests get 10 seconds to finish
 * (application.properties).
 */
@SpringBootApplication
public class MinimartApplication {

  /** Starts the service; exits with status 1 when the configuration is unusable. */
  public static void main(String[] args) {
    AppConfig config;
    try {
      config = AppConfig.fromEnvironment();
    } catch (AppConfig.InvalidConfigException e) {
      System.err.println("minimart: " + e.getMessage());
      System.exit(1);
      return;
    }

    SpringApplication app = new SpringApplication(MinimartApplication.class);
    app.setEnvironment(environmentWithoutOsVariables());
    app.addInitializers(context -> context.getBeanFactory().registerSingleton("appConfig", config));
    app.run(args);
  }

  private static StandardServletEnvironment environmentWithoutOsVariables() {
    StandardServletEnvironment environment = new StandardServletEnvironment();
    environment
        .getPropertySources()
        .remove(StandardEnvironment.SYSTEM_ENVIRONMENT_PROPERTY_SOURCE_NAME);
    return environment;
  }
}
