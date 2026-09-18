package minimart.bdd;

import io.cucumber.java.After;
import io.cucumber.java.Before;
import io.cucumber.java.BeforeAll;
import io.cucumber.java.Scenario;
import java.io.BufferedReader;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.List;

/**
 * The suite's lifecycle: one image build per run, one stack per scenario.
 *
 * <p>Registered through the glue package, like the steps; PicoContainer injects the scenario's
 * {@link ScenarioContext}, the same instance the steps receive.
 */
public final class Hooks {

  private final ScenarioContext context;

  public Hooks(ScenarioContext context) {
    this.context = context;
  }

  /**
   * Builds the service image once for the whole run. Per-scenario containers are then a start, not
   * a build, which is what makes one stack per scenario affordable.
   *
   * <p>It is built exactly the way local/docker-compose.yml builds it: the context is the example
   * root and the Dockerfile is backend/Dockerfile, so the image under test and the image a human
   * runs locally are built from the same two inputs. Rebuilding on every run is also what makes a
   * run test the code in the working tree rather than whatever image was left behind by the last
   * one.
   *
   * <p>The output is copied line by line onto this JVM's stdout rather than inherited: the forked
   * test JVM's native stdout is failsafe's channel, and writing to it directly corrupts it.
   */
  @BeforeAll
  public static void buildServiceImage() throws IOException, InterruptedException {
    Path root = ExamplePaths.exampleRoot();
    List<String> command =
        List.of(
            "docker",
            "build",
            "-f",
            root.resolve("backend").resolve("Dockerfile").toString(),
            "-t",
            ScenarioContext.SERVICE_IMAGE,
            root.toString());
    System.out.println(
        "building " + ScenarioContext.SERVICE_IMAGE + ": " + String.join(" ", command));

    Process build = new ProcessBuilder(command).redirectErrorStream(true).start();
    build.getOutputStream().close();
    try (BufferedReader output = build.inputReader(StandardCharsets.UTF_8)) {
      String line;
      while ((line = output.readLine()) != null) {
        System.out.println(line);
      }
    }
    int exit = build.waitFor();
    if (exit != 0) {
      // Thrown from BeforeAll, this fails the run before any scenario starts: there is nothing to
      // test without the image, and a stale one would be worse than none.
      throw new IllegalStateException(
          "building " + ScenarioContext.SERVICE_IMAGE + " failed: docker build exited " + exit);
    }
  }

  /** Brings up the scenario's network, PostgreSQL and service before its first step. */
  @Before
  public void startInfrastructure() throws Exception {
    context.start();
  }

  /**
   * Tears the scenario down whatever happened — a failed step, or a Before that failed half way.
   *
   * <p>A failed scenario first gets the service container's log attached to its report: the HTTP
   * status a step saw is the symptom, and the stack trace the service logged is usually the cause.
   */
  @After
  public void stopInfrastructure(Scenario scenario) {
    try {
      if (scenario.isFailed()) {
        String logs = context.serviceLogs();
        if (logs != null) {
          scenario.log(
              "── service container log (" + ScenarioContext.SERVICE_IMAGE + ") ──\n" + logs);
        }
      }
    } finally {
      context.stop();
    }
  }
}
