package minimart.bdd;

import io.cucumber.docstring.DocString;
import io.cucumber.java.en.Then;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.stream.Collectors;

/**
 * The response steps of spec/bdd/format.yml: the five assertions and the two steps that save a
 * piece of the response into the context bag.
 *
 * <p>Every single-response step opens with {@link ScenarioContext#requireResponse()}, so after a
 * concurrent step each of them fails saying so instead of reading an answer an earlier step left
 * behind.
 */
public final class ResponseSteps {

  private final ScenarioContext context;

  public ResponseSteps(ScenarioContext context) {
    this.context = context;
  }

  /** {@code response status is <code>}. */
  @Then("^response status is ([1-5][0-9]{2})$")
  public void responseStatusIs(String statusCode) {
    int expected = Integer.parseInt(statusCode);
    ScenarioContext.Response response = context.requireResponse();
    if (response.status() != expected) {
      // A wrong status is almost always explained by the body, so print it.
      throw new AssertionError(
          "expected HTTP "
              + expected
              + ", got "
              + response.status()
              + "\nBody: "
              + response.bodyText());
    }
  }

  /**
   * {@code exactly <n> responses are <status>}.
   *
   * <p>Exact, never "at least": one 201 among four callers is the whole claim a race makes, and an
   * "at least" form would pass on the oversell it exists to catch.
   */
  @Then("^exactly ([0-9]+) responses? (?:is|are) ([1-5][0-9]{2})$")
  public void responseSetStatusCount(String count, String statusCode) {
    int expected = Integer.parseInt(count);
    int status = Integer.parseInt(statusCode);
    List<ScenarioContext.RecordedResponse> set = context.responseSet();
    if (set == null) {
      throw new AssertionError(
          "no concurrent responses recorded: no `is called concurrently:` step has run yet");
    }
    long actual = set.stream().filter(r -> r.status() == status).count();
    if (actual != expected) {
      throw new AssertionError(
          "expected exactly "
              + expected
              + " of the "
              + set.size()
              + " responses to be HTTP "
              + status
              + ", got "
              + actual
              + "\nActual distribution:\n"
              + statusDistribution(set));
    }
  }

  /**
   * The recorded set as a count per status with one body each, so a failed expectation says what
   * really happened.
   */
  private static String statusDistribution(List<ScenarioContext.RecordedResponse> set) {
    Map<Integer, Integer> counts = new TreeMap<>();
    Map<Integer, String> sample = new TreeMap<>();
    for (ScenarioContext.RecordedResponse response : set) {
      counts.merge(response.status(), 1, Integer::sum);
      sample.putIfAbsent(
          response.status(), new String(response.body(), StandardCharsets.UTF_8).strip());
    }
    return counts.entrySet().stream()
        .map(
            e -> "  " + e.getValue() + " × HTTP " + e.getKey() + ", e.g. " + sample.get(e.getKey()))
        .collect(Collectors.joining("\n"));
  }

  /**
   * {@code response body contains:} — a partial match: only the keys present in the docstring are
   * checked and extra keys in the response are ignored. Arrays are matched element-wise and their
   * lengths must be equal, so a list assertion cannot pass against a longer list.
   */
  @Then("^response body contains:$")
  public void responseBodyContains(DocString doc) {
    ScenarioContext.Response response = context.requireResponse();
    String content = context.resolve(doc.getContent());

    Object expected;
    try {
      expected = JsonValues.parse(content);
    } catch (RuntimeException e) {
      throw new AssertionError("expected JSON is not valid: " + e.getMessage(), e);
    }
    Object actual;
    try {
      actual = JsonValues.parse(response.bodyText());
    } catch (RuntimeException e) {
      throw new AssertionError(
          "response body is not valid JSON: " + e.getMessage() + "\nBody: " + response.bodyText(),
          e);
    }

    String failure = JsonValues.matchSubset(expected, actual, "$");
    if (failure != null) {
      throw new AssertionError(
          failure + "\nActual body:\n" + JsonValues.indent(response.bodyText()));
    }
  }

  /**
   * {@code response body does not contain "<text>"}.
   *
   * <p>The check is on the raw body, at any nesting level, whether the text appears as a key or
   * inside a value: proving that {@code token} is absent from a login response means absent, not
   * merely "not a top-level key".
   */
  @Then("^response body does not contain \"([^\"]+)\"$")
  public void responseBodyDoesNotContain(String text) {
    String body = context.requireResponse().bodyText();
    int index = body.indexOf(text);
    if (index < 0) {
      return;
    }
    int start = Math.max(0, index - 40);
    int end = Math.min(body.length(), index + text.length() + 40);
    throw new AssertionError(
        "response body contains \""
            + text
            + "\" but must not: …"
            + body.substring(start, end)
            + "…");
  }

  /**
   * {@code response header "<name>" contains "<substring>"}.
   *
   * <p>The name is matched case-insensitively; the value is a case-sensitive substring. A header
   * that occurs more than once — Set-Cookie does — passes if any occurrence contains the substring.
   */
  @Then("^response header \"([^\"]+)\" contains \"([^\"]+)\"$")
  public void responseHeaderContains(String name, String substring) {
    ScenarioContext.Response response = context.requireResponse();
    List<String> values = response.headerValues(name);
    if (values.isEmpty()) {
      List<String> present = new ArrayList<>(response.headers().keySet());
      present.sort(String.CASE_INSENSITIVE_ORDER);
      throw new AssertionError(
          "response has no \""
              + name
              + "\" header; headers present: "
              + String.join(", ", present));
    }
    for (String value : values) {
      if (value.contains(substring)) {
        return;
      }
    }
    throw new AssertionError(
        "no \"" + name + "\" header contains \"" + substring + "\"; values: " + quoted(values));
  }

  /**
   * {@code save response body field "<field>" as "<varName>"}.
   *
   * <p>The value is stored as a string so the same variable interpolates into JSON and into SQL.
   * Numbers are rendered without quotes or exponent (1 → "1").
   */
  @Then("^save response body field \"([a-zA-Z_][a-zA-Z0-9_]*)\" as \"([a-z][a-zA-Z0-9]+)\"$")
  public void saveResponseBodyField(String field, String varName) {
    ScenarioContext.Response response = context.requireResponse();
    Object body;
    try {
      body = JsonValues.parse(response.bodyText());
    } catch (RuntimeException e) {
      throw new AssertionError(
          "response body is not a JSON object: "
              + e.getMessage()
              + "\nBody: "
              + response.bodyText(),
          e);
    }
    Map<?, ?> object;
    if (body == null) {
      // A JSON null decodes to no object at all, and so to no field — as it does in go-nuxt.
      object = Map.of();
    } else if (body instanceof Map<?, ?> map) {
      object = map;
    } else {
      throw new AssertionError("response body is not a JSON object\nBody: " + response.bodyText());
    }
    if (!object.containsKey(field)) {
      throw new AssertionError(
          "response body has no field \""
              + field
              + "\"\nBody: "
              + JsonValues.indent(response.bodyText()));
    }
    Object value = object.get(field);
    if (value == null) {
      throw new AssertionError("response body field \"" + field + "\" is null");
    }
    context.saveVar(varName, JsonValues.renderValue(value));
  }

  /**
   * {@code save response cookie "<name>" as "<varName>"}.
   *
   * <p>The other half of save response body field: the same contract — produce a value, name it,
   * change nothing else — for the part of a response that is not the body. It is how a scenario
   * carries a session, because a login is written out as the ordinary request it is and the token
   * is in the Set-Cookie header, not in the body the login answers with.
   *
   * <p>Saving a token authenticates nothing: the client keeps no cookie jar, so a later request is
   * made as somebody only when its own docstring writes the header.
   */
  @Then("^save response cookie \"([a-zA-Z0-9_-]+)\" as \"([a-z][a-zA-Z0-9]+)\"$")
  public void saveResponseCookie(String name, String varName) {
    ScenarioContext.Response response = context.requireResponse();
    List<String> setCookies = response.headerValues("Set-Cookie");
    for (String header : setCookies) {
      // name=value is everything before the first ';'; the attributes after it are not the value.
      String pair = header.strip();
      int semicolon = pair.indexOf(';');
      if (semicolon >= 0) {
        pair = pair.substring(0, semicolon);
      }
      int equals = pair.indexOf('=');
      if (equals < 0 || !pair.substring(0, equals).strip().equals(name)) {
        continue;
      }
      String value = pair.substring(equals + 1).strip();
      if (value.length() >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
        value = value.substring(1, value.length() - 1);
      }
      // An empty value is how a session is CLEARED — the service expires the session hint with
      // exactly that. Saving it would name a credential that authenticates nothing while every
      // request below reads as though it does, which is the one failure this step exists to make
      // impossible.
      if (value.isEmpty()) {
        throw new AssertionError(
            "response set cookie \""
                + name
                + "\" to the empty string, which clears it rather than granting one");
      }
      context.saveVar(varName, value);
      return;
    }
    throw new AssertionError(
        "response set no \"" + name + "\" cookie (Set-Cookie: " + setCookies + ")");
  }

  private static String quoted(List<String> values) {
    return values.stream().map(v -> "\"" + v + "\"").collect(Collectors.joining(" ", "[", "]"));
  }
}
