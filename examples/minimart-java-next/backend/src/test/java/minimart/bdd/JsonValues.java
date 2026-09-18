package minimart.bdd;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.TreeMap;
import tools.jackson.databind.DeserializationFeature;
import tools.jackson.databind.json.JsonMapper;

/**
 * JSON as the steps see it: decoded into plain values, compared by the partial-match rules of
 * format.yml, and rendered back for failure messages.
 *
 * <p>Values are decoded the way go-nuxt's {@code json.Unmarshal} into {@code any} decodes them —
 * objects to maps, arrays to lists, and EVERY number to a double. That last part is deliberate: it
 * makes {@code 1} and {@code 1.0} equal, exactly as they are to go-nuxt's float64 comparison, so
 * the two runners pass and fail on the same bodies.
 */
final class JsonValues {

  /**
   * Trailing content after the one JSON value is an error, as it is for {@code json.Unmarshal}: a
   * docstring with a stray second value is a mistake worth hearing about.
   */
  static final JsonMapper MAPPER =
      JsonMapper.builder().enable(DeserializationFeature.FAIL_ON_TRAILING_TOKENS).build();

  /** The special value that matches anything present and not null. */
  static final String NON_NULL = "<non-null>";

  private JsonValues() {}

  /**
   * Decodes one JSON text into maps, lists, strings, doubles, booleans and null.
   *
   * @throws tools.jackson.core.JacksonException if the text is not exactly one JSON value
   */
  static Object parse(String text) {
    return normalize(MAPPER.readValue(text, Object.class));
  }

  private static Object normalize(Object value) {
    if (value instanceof Number number) {
      return number.doubleValue();
    }
    if (value instanceof Map<?, ?> map) {
      Map<String, Object> out = new LinkedHashMap<>();
      map.forEach((k, v) -> out.put(String.valueOf(k), normalize(v)));
      return out;
    }
    if (value instanceof List<?> list) {
      List<Object> out = new ArrayList<>(list.size());
      list.forEach(v -> out.add(normalize(v)));
      return out;
    }
    return value;
  }

  /**
   * Reports whether actual satisfies expected under the partial-match rules of format.yml, naming
   * the first path that does not. Returns null on a match.
   *
   * <ul>
   *   <li>objects: every key of expected must exist in actual and match; extra keys in actual are
   *       ignored.
   *   <li>arrays: matched element-wise, lengths must be equal.
   *   <li>"&lt;non-null&gt;": the field must be present and not JSON null; nothing else is
   *       constrained.
   *   <li>anything else: equality, numbers compared as doubles.
   * </ul>
   */
  static String matchSubset(Object expected, Object actual, String path) {
    if (NON_NULL.equals(expected)) {
      return actual == null ? path + ": expected a non-null value, got null" : null;
    }

    if (expected instanceof Map<?, ?> want) {
      if (!(actual instanceof Map<?, ?> got)) {
        return path + ": expected an object, got " + describe(actual);
      }
      // Sorted so the first reported failure does not depend on the docstring's key order.
      for (String key : new TreeMap<>(stringKeys(want)).keySet()) {
        if (!got.containsKey(key)) {
          return path + "." + key + ": missing from the response";
        }
        String failure = matchSubset(want.get(key), got.get(key), path + "." + key);
        if (failure != null) {
          return failure;
        }
      }
      return null;
    }

    if (expected instanceof List<?> want) {
      if (!(actual instanceof List<?> got)) {
        return path + ": expected an array, got " + describe(actual);
      }
      if (got.size() != want.size()) {
        return path + ": expected " + want.size() + " element(s), got " + got.size();
      }
      for (int i = 0; i < want.size(); i++) {
        String failure = matchSubset(want.get(i), got.get(i), path + "[" + i + "]");
        if (failure != null) {
          return failure;
        }
      }
      return null;
    }

    if (!scalarEquals(expected, actual)) {
      return path + ": expected " + describe(expected) + ", got " + describe(actual);
    }
    return null;
  }

  private static boolean scalarEquals(Object expected, Object actual) {
    if (expected instanceof Double want && actual instanceof Double got) {
      // ==, not Double.equals: 0 and -0 are the same JSON number.
      return want.doubleValue() == got.doubleValue();
    }
    return Objects.equals(expected, actual);
  }

  private static Map<String, Object> stringKeys(Map<?, ?> map) {
    Map<String, Object> out = new LinkedHashMap<>();
    map.forEach((k, v) -> out.put(String.valueOf(k), v));
    return out;
  }

  /**
   * Renders a decoded value as compact JSON with sorted keys, the way go-nuxt's {@code
   * json.Marshal} renders one in a failure message.
   */
  static String describe(Object value) {
    StringBuilder out = new StringBuilder();
    render(value, out, null, 0);
    return out.toString();
  }

  /**
   * Pretty-prints a response body with sorted keys and two-space indentation, or returns it
   * unchanged when it is not JSON at all.
   */
  static String indent(String raw) {
    Object value;
    try {
      value = parse(raw);
    } catch (RuntimeException e) {
      return raw;
    }
    StringBuilder out = new StringBuilder();
    render(value, out, "  ", 0);
    return out.toString();
  }

  /**
   * Turns a decoded value into the string a context variable holds, so the same variable
   * interpolates into JSON and into SQL. Numbers are rendered without quotes or exponent (1 → "1").
   */
  static String renderValue(Object value) {
    if (value instanceof String s) {
      return s;
    }
    if (value instanceof Boolean b) {
      return b.toString();
    }
    if (value instanceof Double d) {
      if (d == Math.rint(d) && Math.abs(d) < 9.0e18) {
        return Long.toString(d.longValue());
      }
      return new BigDecimal(Double.toString(d)).toPlainString();
    }
    return describe(value);
  }

  private static void render(Object value, StringBuilder out, String indent, int depth) {
    if (value == null) {
      out.append("null");
    } else if (value instanceof String s) {
      out.append(MAPPER.writeValueAsString(s));
    } else if (value instanceof Double d) {
      out.append(number(d));
    } else if (value instanceof Map<?, ?> map) {
      if (map.isEmpty()) {
        out.append("{}");
        return;
      }
      out.append('{');
      boolean first = true;
      for (Map.Entry<String, Object> e : new TreeMap<>(stringKeys(map)).entrySet()) {
        if (!first) {
          out.append(',');
        }
        first = false;
        newline(out, indent, depth + 1);
        out.append(MAPPER.writeValueAsString(e.getKey())).append(indent == null ? ":" : ": ");
        render(e.getValue(), out, indent, depth + 1);
      }
      newline(out, indent, depth);
      out.append('}');
    } else if (value instanceof List<?> list) {
      if (list.isEmpty()) {
        out.append("[]");
        return;
      }
      out.append('[');
      for (int i = 0; i < list.size(); i++) {
        if (i > 0) {
          out.append(',');
        }
        newline(out, indent, depth + 1);
        render(list.get(i), out, indent, depth + 1);
      }
      newline(out, indent, depth);
      out.append(']');
    } else {
      out.append(value);
    }
  }

  private static void newline(StringBuilder out, String indent, int depth) {
    if (indent != null) {
      out.append('\n').append(indent.repeat(depth));
    }
  }

  /** A double the way Go's encoder writes a float64: integral values without a fraction. */
  private static String number(double d) {
    double abs = Math.abs(d);
    if (abs != 0 && (abs < 1e-6 || abs >= 1e21)) {
      return Double.toString(d).replace('E', 'e');
    }
    return new BigDecimal(Double.toString(d)).stripTrailingZeros().toPlainString();
  }
}
