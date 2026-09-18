package minimart.testsupport;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import org.springframework.mock.web.MockHttpServletResponse;

/** Reads Set-Cookie headers as they were written, attribute by attribute. */
public final class SetCookies {

  private SetCookies() {}

  /**
   * One Set-Cookie header, parsed.
   *
   * @param attributes lower-cased attribute name → value ("" for a flag such as HttpOnly)
   */
  public record SetCookie(String name, String value, Map<String, String> attributes) {

    /** Whether the attribute is present at all. */
    public boolean has(String attribute) {
      return attributes.containsKey(attribute.toLowerCase(Locale.ROOT));
    }

    /** The attribute's value, or null when absent. */
    public String get(String attribute) {
      return attributes.get(attribute.toLowerCase(Locale.ROOT));
    }
  }

  /** Every Set-Cookie on {@code response} named {@code name}. */
  public static List<SetCookie> named(MockHttpServletResponse response, String name) {
    return response.getHeaders("Set-Cookie").stream()
        .map(SetCookies::parse)
        .filter(cookie -> cookie.name().equals(name))
        .toList();
  }

  static SetCookie parse(String header) {
    String[] parts = header.split(";");
    String[] pair = parts[0].trim().split("=", 2);
    Map<String, String> attributes = new LinkedHashMap<>();
    for (int i = 1; i < parts.length; i++) {
      String[] attribute = parts[i].trim().split("=", 2);
      attributes.put(
          attribute[0].toLowerCase(Locale.ROOT), attribute.length > 1 ? attribute[1] : "");
    }
    return new SetCookie(pair[0], pair.length > 1 ? pair[1] : "", attributes);
  }
}
