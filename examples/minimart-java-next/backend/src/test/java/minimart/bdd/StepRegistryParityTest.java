package minimart.bdd;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;

import io.cucumber.java.StepDefinitionAnnotation;
import io.cucumber.java.StepDefinitionAnnotations;
import java.io.IOException;
import java.io.InputStream;
import java.lang.annotation.Annotation;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Enumeration;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;
import org.yaml.snakeyaml.LoaderOptions;
import org.yaml.snakeyaml.Yaml;
import org.yaml.snakeyaml.constructor.SafeConstructor;

/**
 * spec/bdd/format.yml is the grammar; the step annotations in this package must be it, verbatim.
 *
 * <p>For each of the twelve step ids the API surface implements, the registry's {@code pattern}
 * must be the value of exactly one step annotation, and there must be no other step annotation at
 * all. A pattern paraphrased in Java — an escaped quote dropped, a character class widened — would
 * be a second grammar that agrees with the first only on the features that happen to exist today.
 * The Cucumber run would not notice: it only proves the features match SOME step.
 *
 * <p>Runs under surefire, with no Docker: it reads a YAML file and reflects over classes.
 */
class StepRegistryParityTest {

  /** The registry ids this runner implements: the API steps and the three shared ones. */
  private static final List<String> API_STEP_IDS =
      List.of(
          "run_migration",
          "exec_postgresql",
          "http_request",
          "http_request_concurrent",
          "response_status",
          "response_set_status_count",
          "response_body_contains",
          "response_body_does_not_contain",
          "response_header_contains",
          "save_response_body_field",
          "save_response_cookie",
          "postgresql_query_returns");

  private static final String GLUE_PACKAGE = "minimart.bdd";

  @Test
  void everyApiStepPatternIsImplementedVerbatimExactlyOnceAndNothingElseIs() throws Exception {
    Map<String, String> registry = registryPatterns();
    List<String> annotated = stepAnnotationValues();

    for (String id : API_STEP_IDS) {
      String pattern = registry.get(id);
      assertNotNull(pattern, "spec/bdd/format.yml has no step \"" + id + "\"");
      assertEquals(
          1,
          Collections.frequency(annotated, pattern),
          "step \""
              + id
              + "\": its registry pattern must be the value of exactly one step annotation in "
              + GLUE_PACKAGE
              + "\n  registry pattern: "
              + pattern
              + "\n  annotations:\n    "
              + String.join("\n    ", annotated));
    }
    assertEquals(
        API_STEP_IDS.size(),
        annotated.size(),
        "every step annotation in "
            + GLUE_PACKAGE
            + " must implement one of the registry's API steps, and none twice; found:\n    "
            + String.join("\n    ", annotated));
  }

  /** id → pattern for every step in spec/bdd/format.yml. */
  private static Map<String, String> registryPatterns() throws IOException {
    Path format = ExamplePaths.formatRegistry();
    Object document;
    try (InputStream in = Files.newInputStream(format)) {
      document = new Yaml(new SafeConstructor(new LoaderOptions())).load(in);
    }
    Map<String, String> patterns = new LinkedHashMap<>();
    if (document instanceof Map<?, ?> root && root.get("steps") instanceof List<?> steps) {
      for (Object step : steps) {
        if (step instanceof Map<?, ?> entry
            && entry.get("id") instanceof String id
            && entry.get("pattern") instanceof String pattern) {
          patterns.put(id, pattern);
        }
      }
    }
    assertFalse(patterns.isEmpty(), format + " declares no steps");
    return patterns;
  }

  /** The value of every Cucumber step annotation on every method of every class in the package. */
  private static List<String> stepAnnotationValues() throws Exception {
    List<String> values = new ArrayList<>();
    for (Class<?> type : gluePackageClasses()) {
      for (Method method : type.getDeclaredMethods()) {
        for (Annotation annotation : method.getAnnotations()) {
          collect(annotation, values);
        }
      }
    }
    Collections.sort(values);
    return values;
  }

  /**
   * Adds the pattern of a step annotation — Given, When, Then, And, But, or any other language's,
   * all of which Cucumber marks with StepDefinitionAnnotation — and of each annotation inside a
   * repeated-annotation container.
   */
  private static void collect(Annotation annotation, List<String> values)
      throws ReflectiveOperationException {
    Class<? extends Annotation> type = annotation.annotationType();
    if (type.isAnnotationPresent(StepDefinitionAnnotation.class)) {
      values.add((String) value(annotation));
    } else if (type.isAnnotationPresent(StepDefinitionAnnotations.class)) {
      for (Annotation repeated : (Annotation[]) value(annotation)) {
        collect(repeated, values);
      }
    }
  }

  private static Object value(Annotation annotation) throws ReflectiveOperationException {
    try {
      return annotation.annotationType().getMethod("value").invoke(annotation);
    } catch (InvocationTargetException e) {
      throw new IllegalStateException("reading " + annotation, e.getCause());
    }
  }

  /**
   * Every class compiled into the glue package, found on disk under each classpath root that holds
   * it. Loaded without initialisation: only their annotations are read.
   */
  private static List<Class<?>> gluePackageClasses()
      throws IOException, URISyntaxException, ClassNotFoundException {
    ClassLoader loader = StepRegistryParityTest.class.getClassLoader();
    String resource = GLUE_PACKAGE.replace('.', '/');
    List<Class<?>> classes = new ArrayList<>();
    Enumeration<URL> roots = loader.getResources(resource);
    while (roots.hasMoreElements()) {
      URL root = roots.nextElement();
      if (!"file".equals(root.getProtocol())) {
        continue;
      }
      Path dir = Path.of(root.toURI());
      List<Path> files;
      try (Stream<Path> listing = Files.list(dir)) {
        files = listing.filter(p -> p.getFileName().toString().endsWith(".class")).toList();
      }
      for (Path file : files) {
        String simple = file.getFileName().toString();
        String name = GLUE_PACKAGE + "." + simple.substring(0, simple.length() - ".class".length());
        classes.add(Class.forName(name, false, loader));
      }
    }
    assertFalse(classes.isEmpty(), "no compiled classes found for package " + GLUE_PACKAGE);
    return classes;
  }
}
