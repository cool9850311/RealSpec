package minimart.bdd;

import static io.cucumber.junit.platform.engine.Constants.GLUE_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.JUNIT_PLATFORM_NAMING_STRATEGY_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.PARALLEL_CONFIG_CUSTOM_CLASS_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.PARALLEL_CONFIG_STRATEGY_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.PARALLEL_EXECUTION_ENABLED_PROPERTY_NAME;
import static io.cucumber.junit.platform.engine.Constants.PLUGIN_PROPERTY_NAME;

import org.junit.platform.suite.api.ConfigurationParameter;
import org.junit.platform.suite.api.IncludeEngines;
import org.junit.platform.suite.api.SelectDirectories;
import org.junit.platform.suite.api.Suite;

/**
 * Runs spec/bdd/api/*.feature against this service — the Cucumber-JVM counterpart of go-nuxt's
 * TestAPIFeatures.
 *
 * <p>Failsafe runs it during {@code mvnw verify} (its name ends in {@code IT}); surefire never
 * does, so {@code mvnw test} stays the docker-free unit suite.
 *
 * <p>Every setting that selects or shapes the Cucumber run lives here, on the suite, rather than in
 * junit-platform.properties. That file is read by every JUnit Platform launch in this module,
 * surefire's included, and the Cucumber engine is on surefire's classpath too: a global {@code
 * cucumber.features} there would make {@code mvnw test} start the whole docker suite.
 *
 * <p>The feature directory is relative to the working directory, which failsafe sets to backend/.
 * If it ever resolved to nothing, {@code @Suite}'s default {@code failIfNoTests = true} fails the
 * run instead of reporting zero scenarios as green.
 *
 * <p>Operator overrides that need no code change, all read from system properties:
 *
 * <ul>
 *   <li>{@code -Dcucumber.filter.tags="@orders and not @race"} — a tag expression (the analogue of
 *       GODOG_TAGS);
 *   <li>{@code -Dcucumber.features=../spec/bdd/api/auth.feature} — replaces the selection below
 *       (the analogue of GODOG_PATHS);
 *   <li>{@code CUCUMBER_CONCURRENCY=<n>} (environment) — see {@link HalfTheProcessorsStrategy}.
 * </ul>
 *
 * <p>Undefined and pending steps fail the scenario: Cucumber-JVM 7 has no non-strict mode, so a
 * step the registry declares and nobody implemented cannot pass as a yellow line.
 */
@Suite
@IncludeEngines("cucumber")
@SelectDirectories("../spec/bdd/api")
@ConfigurationParameter(key = GLUE_PROPERTY_NAME, value = "minimart.bdd")
@ConfigurationParameter(
    key = PLUGIN_PROPERTY_NAME,
    value =
        "pretty, html:target/cucumber-reports/cucumber.html,"
            + " junit:target/cucumber-reports/cucumber.xml")
// "long" names a scenario "Feature - Scenario" (and an outline row by its example) in the
// failsafe reports, instead of a bare "Example #1.2".
@ConfigurationParameter(key = JUNIT_PLATFORM_NAMING_STRATEGY_PROPERTY_NAME, value = "long")
@ConfigurationParameter(key = PARALLEL_EXECUTION_ENABLED_PROPERTY_NAME, value = "true")
@ConfigurationParameter(key = PARALLEL_CONFIG_STRATEGY_PROPERTY_NAME, value = "custom")
@ConfigurationParameter(
    key = PARALLEL_CONFIG_CUSTOM_CLASS_PROPERTY_NAME,
    value = "minimart.bdd.HalfTheProcessorsStrategy")
public class RunApiFeaturesIT {}
