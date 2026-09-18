package minimart.bdd;

import java.util.concurrent.ForkJoinPool;
import java.util.function.Predicate;
import org.junit.platform.engine.ConfigurationParameters;
import org.junit.platform.engine.support.hierarchical.ParallelExecutionConfiguration;
import org.junit.platform.engine.support.hierarchical.ParallelExecutionConfigurationStrategy;

/**
 * How many scenarios run at once: half the processors, at least one, unless CUCUMBER_CONCURRENCY
 * says otherwise.
 *
 * <p>One unit of parallelism here is a stack of containers — a PostgreSQL and a JVM service — not a
 * thread, so the processor count is halved, as go-nuxt halves GOMAXPROCS. The JUnit "dynamic"
 * strategy cannot say that (its factor is applied to all processors and it rounds differently), and
 * "fixed" cannot read the environment, hence a strategy of our own.
 *
 * <p>Pool size, core size and minimum-runnable are all pinned to the same number: a larger pool
 * would let the ForkJoinPool start more scenarios than the machine was sized for, which on a
 * container-per-scenario suite turns into startup timeouts rather than speed.
 */
public final class HalfTheProcessorsStrategy implements ParallelExecutionConfigurationStrategy {

  /** Environment override: a positive integer. */
  static final String CONCURRENCY_ENV = "CUCUMBER_CONCURRENCY";

  /** Required by the JUnit Platform, which instantiates the strategy reflectively. */
  public HalfTheProcessorsStrategy() {}

  @Override
  public ParallelExecutionConfiguration createConfiguration(ConfigurationParameters parameters) {
    int processors = Runtime.getRuntime().availableProcessors();
    int concurrency = concurrency(System.getenv(CONCURRENCY_ENV), processors);
    System.out.printf(
        "running scenarios at concurrency %d (availableProcessors=%d)%n", concurrency, processors);
    return new Fixed(concurrency);
  }

  /**
   * Resolves the concurrency from the raw environment value.
   *
   * <p>Unlike go-nuxt's envInt, a value that is set but is not a positive integer is an error
   * rather than a silent fallback: a typo in CI would otherwise run the suite at a concurrency
   * nobody chose, and look exactly like a run that honoured it.
   */
  static int concurrency(String raw, int processors) {
    if (raw == null || raw.isBlank()) {
      return Math.max(1, processors / 2);
    }
    int value;
    try {
      value = Integer.parseInt(raw.trim());
    } catch (NumberFormatException e) {
      throw new IllegalArgumentException(
          CONCURRENCY_ENV + " must be a positive integer, got \"" + raw + "\"", e);
    }
    if (value < 1) {
      throw new IllegalArgumentException(
          CONCURRENCY_ENV + " must be a positive integer, got \"" + raw + "\"");
    }
    return value;
  }

  private record Fixed(int parallelism) implements ParallelExecutionConfiguration {

    @Override
    public int getParallelism() {
      return parallelism;
    }

    @Override
    public int getMinimumRunnable() {
      return parallelism;
    }

    @Override
    public int getMaxPoolSize() {
      return parallelism;
    }

    @Override
    public int getCorePoolSize() {
      return parallelism;
    }

    @Override
    public int getKeepAliveSeconds() {
      return 30;
    }

    /**
     * Saturated means "carry on without a compensating thread" instead of throwing
     * RejectedExecutionException when every worker is blocked — which, with each worker waiting on
     * Docker, is the normal state of this pool rather than an error.
     */
    @Override
    public Predicate<? super ForkJoinPool> getSaturatePredicate() {
      return pool -> true;
    }
  }
}
