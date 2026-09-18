package minimart.bdd;

import java.net.URISyntaxException;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Locates examples/minimart-java-next on disk.
 *
 * <p>The search starts from where this class was loaded (backend/target/test-classes) rather than
 * from the working directory, so the image build and the registry parity test find the same files
 * whether they run under Maven, from an IDE, or from the repository root. The working directory is
 * the fallback for the unusual case of classes loaded from a jar.
 */
final class ExamplePaths {

  private ExamplePaths() {}

  /** The example root: the directory holding both backend/pom.xml and spec/bdd/format.yml. */
  static Path exampleRoot() {
    Path fromClasses = classesLocation();
    if (fromClasses != null) {
      Path found = searchUpwards(fromClasses);
      if (found != null) {
        return found;
      }
    }
    Path cwd = Path.of("").toAbsolutePath();
    Path found = searchUpwards(cwd);
    if (found != null) {
      return found;
    }
    throw new IllegalStateException(
        "cannot locate examples/minimart-java-next (a directory holding backend/pom.xml and"
            + " spec/bdd/format.yml) above "
            + (fromClasses != null ? fromClasses : cwd));
  }

  /** spec/bdd/format.yml of this example. */
  static Path formatRegistry() {
    return exampleRoot().resolve("spec").resolve("bdd").resolve("format.yml");
  }

  private static Path classesLocation() {
    try {
      var source = ExamplePaths.class.getProtectionDomain().getCodeSource();
      return source == null ? null : Path.of(source.getLocation().toURI()).toAbsolutePath();
    } catch (URISyntaxException | IllegalArgumentException | SecurityException e) {
      return null;
    }
  }

  private static Path searchUpwards(Path start) {
    for (Path dir = start; dir != null; dir = dir.getParent()) {
      if (Files.isRegularFile(dir.resolve("backend").resolve("pom.xml"))
          && Files.isRegularFile(dir.resolve("spec").resolve("bdd").resolve("format.yml"))) {
        return dir;
      }
    }
    return null;
  }
}
