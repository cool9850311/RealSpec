package minimart.bdd;

import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import tools.jackson.core.JacksonException;
import tools.jackson.databind.JsonNode;
import tools.jackson.databind.node.JsonNodeType;

/**
 * The docstring of an http_request step, and one element of an http_request_concurrent array:
 * {@code "headers"} and {@code "body"}, both optional, nothing else allowed.
 *
 * @param headers the request headers the docstring writes, in its order
 * @param body the request body to send as compact JSON, or null to send none
 */
record RequestEnvelope(Map<String, String> headers, String body) {

  /**
   * Headers the transport writes from the request itself. java.net.http refuses to let a caller set
   * them, and the concurrent step's hand-written request writes them too; rejecting them here gives
   * both steps the same, readable failure instead of two different ones.
   */
  private static final Set<String> TRANSPORT_HEADERS =
      Set.of("connection", "content-length", "expect", "host", "transfer-encoding", "upgrade");

  RequestEnvelope {
    headers = Collections.unmodifiableMap(new LinkedHashMap<>(headers));
  }

  /** Decodes an http_request docstring. */
  static RequestEnvelope parseSingle(String content) {
    try {
      return fromNode(JsonValues.MAPPER.readTree(content));
    } catch (JacksonException | IllegalArgumentException e) {
      throw new AssertionError(
          "request docstring must be a JSON object with only \"headers\" and \"body\": "
              + e.getMessage(),
          e);
    }
  }

  /** Decodes an http_request_concurrent docstring: one envelope per caller. */
  static List<RequestEnvelope> parseArray(String content) {
    try {
      JsonNode root = JsonValues.MAPPER.readTree(content);
      List<RequestEnvelope> envelopes = new ArrayList<>();
      if (root == null || root.isNull() || root.isMissingNode()) {
        // Absent or null decodes to no callers, which the step then refuses by count.
        return envelopes;
      }
      if (!root.isArray()) {
        throw new IllegalArgumentException("expected a JSON array, got " + describe(root));
      }
      for (int i = 0; i < root.size(); i++) {
        envelopes.add(fromNode(root.get(i)));
      }
      return envelopes;
    } catch (JacksonException | IllegalArgumentException e) {
      throw new AssertionError(
          "concurrent request docstring must be a JSON array of objects with only \"headers\" and"
              + " \"body\": "
              + e.getMessage(),
          e);
    }
  }

  private static RequestEnvelope fromNode(JsonNode node) {
    if (node == null || node.isMissingNode()) {
      throw new IllegalArgumentException("the docstring is empty");
    }
    Map<String, String> headers = new LinkedHashMap<>();
    if (node.isNull()) {
      // go-nuxt's decoder turns a JSON null into the empty envelope; so does this one.
      return new RequestEnvelope(headers, null);
    }
    if (!node.isObject()) {
      throw new IllegalArgumentException("expected a JSON object, got " + describe(node));
    }
    String body = null;
    for (Map.Entry<String, JsonNode> field : node.properties()) {
      switch (field.getKey()) {
        case "headers" -> readHeaders(field.getValue(), headers);
        case "body" -> body = bodyText(field.getValue());
        default -> throw new IllegalArgumentException("unknown field \"" + field.getKey() + "\"");
      }
    }
    return new RequestEnvelope(headers, body);
  }

  private static void readHeaders(JsonNode node, Map<String, String> headers) {
    if (node.isNull()) {
      return;
    }
    if (!node.isObject()) {
      throw new IllegalArgumentException("\"headers\" must be an object, got " + describe(node));
    }
    for (Map.Entry<String, JsonNode> header : node.properties()) {
      String name = header.getKey();
      JsonNode value = header.getValue();
      if (value.getNodeType() != JsonNodeType.STRING) {
        throw new IllegalArgumentException(
            "header \"" + name + "\" must be a string, got " + describe(value));
      }
      if (TRANSPORT_HEADERS.contains(name.toLowerCase(Locale.ROOT))) {
        throw new IllegalArgumentException(
            "header \""
                + name
                + "\" is written by the client from the request itself and cannot be set in a"
                + " docstring");
      }
      String text = value.asString();
      if (text.indexOf('\r') >= 0 || text.indexOf('\n') >= 0) {
        throw new IllegalArgumentException("header \"" + name + "\" contains a line break");
      }
      headers.put(name, text);
    }
  }

  /**
   * An absent body, {} or null all mean "send none", so a GET that needs neither headers nor body
   * can still carry the {} docstring the grammar requires. Anything else is sent as compact JSON —
   * the same value the docstring wrote, re-encoded, which is what a JSON-reading service sees
   * either way.
   */
  private static String bodyText(JsonNode node) {
    if (node.isNull() || (node.isObject() && node.isEmpty())) {
      return null;
    }
    return JsonValues.MAPPER.writeValueAsString(node);
  }

  private static String describe(JsonNode node) {
    return node.getNodeType().name().toLowerCase(Locale.ROOT);
  }

  /**
   * The headers to send: Content-Type application/json when there is a body, then the docstring's
   * own headers, which override it (names compared case-insensitively, as HTTP compares them).
   */
  Map<String, String> effectiveHeaders() {
    Map<String, String> effective = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
    if (body != null) {
      effective.put("Content-Type", "application/json");
    }
    effective.putAll(headers);
    return effective;
  }
}
