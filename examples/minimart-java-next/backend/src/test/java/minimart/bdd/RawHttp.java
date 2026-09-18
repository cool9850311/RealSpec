package minimart.bdd;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;

/**
 * HTTP/1.1 written and read by hand, for the concurrent step.
 *
 * <p>The concurrent step cannot hand its callers to an HTTP client: a client connects when it is
 * asked to send, and the step's whole point is that by the time the gate opens there is nothing
 * left to do but write. So each request is serialised to bytes up front, onto a socket opened up
 * front, and the answer is read back to EOF — {@code Connection: close} makes one request per
 * connection, and the server's close is the end of the answer.
 */
final class RawHttp {

  private RawHttp() {}

  /**
   * Serialises one request, as go-nuxt's {@code http.Request.Write} would: request line, Host,
   * {@code Connection: close}, Content-Length, then the envelope's headers, a blank line and the
   * body.
   *
   * <p>Content-Length is written whenever there is a body, and also for a body-less POST, PUT,
   * PATCH or DELETE — as Go does, because servers commonly expect one on those methods. A body-less
   * GET carries none.
   */
  static byte[] serialize(
      String method, String path, String authority, Map<String, String> headers, String body) {
    byte[] payload = body == null ? new byte[0] : body.getBytes(StandardCharsets.UTF_8);
    StringBuilder head = new StringBuilder();
    head.append(method).append(' ').append(path).append(" HTTP/1.1\r\n");
    head.append("Host: ").append(authority).append("\r\n");
    head.append("Connection: close\r\n");
    if (body != null || !method.equals("GET")) {
      head.append("Content-Length: ").append(payload.length).append("\r\n");
    }
    headers.forEach((name, value) -> head.append(name).append(": ").append(value).append("\r\n"));
    head.append("\r\n");

    byte[] headBytes = head.toString().getBytes(StandardCharsets.UTF_8);
    byte[] request = Arrays.copyOf(headBytes, headBytes.length + payload.length);
    System.arraycopy(payload, 0, request, headBytes.length, payload.length);
    return request;
  }

  /**
   * Reads the socket to EOF, never past {@code deadlineNanos} (a {@link System#nanoTime()} value):
   * the read timeout is re-armed before every read with whatever time is left, so the bound is on
   * the whole answer, not on each packet of it.
   */
  static byte[] readToEof(Socket socket, long deadlineNanos) throws IOException {
    InputStream in = socket.getInputStream();
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    byte[] buffer = new byte[8192];
    while (true) {
      long remainingMillis = (deadlineNanos - System.nanoTime()) / 1_000_000;
      if (remainingMillis <= 0) {
        throw new SocketTimeoutException("no complete response before the deadline");
      }
      socket.setSoTimeout((int) Math.min(Integer.MAX_VALUE, remainingMillis));
      int n = in.read(buffer);
      if (n < 0) {
        return out.toByteArray();
      }
      out.write(buffer, 0, n);
    }
  }

  /**
   * Parses one complete HTTP/1.x response: status line, headers, and a body delimited by chunked
   * transfer coding, by Content-Length, or by the end of the connection. Interim 1xx responses
   * before the final one are skipped, as an HTTP client would.
   */
  static ScenarioContext.RecordedResponse parse(byte[] raw) throws IOException {
    int start = 0;
    while (true) {
      int headEnd = indexOf(raw, "\r\n\r\n".getBytes(StandardCharsets.ISO_8859_1), start);
      if (headEnd < 0) {
        throw new IOException("malformed response: the connection closed before the headers ended");
      }
      String[] lines =
          new String(raw, start, headEnd - start, StandardCharsets.ISO_8859_1).split("\r\n");
      int status = statusCode(lines[0]);
      Map<String, List<String>> headers = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
      for (int i = 1; i < lines.length; i++) {
        int colon = lines[i].indexOf(':');
        if (colon <= 0) {
          throw new IOException("malformed response header line: " + lines[i]);
        }
        headers
            .computeIfAbsent(lines[i].substring(0, colon).trim(), k -> new ArrayList<>())
            .add(lines[i].substring(colon + 1).trim());
      }
      int bodyStart = headEnd + 4;

      if (status >= 100 && status < 200 && status != 101) {
        start = bodyStart;
        continue;
      }
      return new ScenarioContext.RecordedResponse(status, body(raw, bodyStart, status, headers));
    }
  }

  private static int statusCode(String statusLine) throws IOException {
    // "HTTP/1.1 201 Created" — the reason phrase is optional and may contain spaces.
    String[] parts = statusLine.split(" ", 3);
    if (parts.length < 2 || !parts[0].startsWith("HTTP/1.")) {
      throw new IOException("malformed status line: " + statusLine);
    }
    try {
      int status = Integer.parseInt(parts[1]);
      if (status < 100 || status > 999) {
        throw new IOException("malformed status line: " + statusLine);
      }
      return status;
    } catch (NumberFormatException e) {
      throw new IOException("malformed status line: " + statusLine, e);
    }
  }

  private static byte[] body(
      byte[] raw, int bodyStart, int status, Map<String, List<String>> headers) throws IOException {
    if (status == 204 || status == 304) {
      return new byte[0];
    }
    List<String> transferEncoding = headers.getOrDefault("Transfer-Encoding", List.of());
    if (transferEncoding.stream().anyMatch(v -> v.toLowerCase(Locale.ROOT).contains("chunked"))) {
      return dechunk(raw, bodyStart);
    }
    List<String> contentLength = headers.getOrDefault("Content-Length", List.of());
    if (!contentLength.isEmpty()) {
      long length;
      try {
        length = Long.parseLong(contentLength.get(0));
      } catch (NumberFormatException e) {
        throw new IOException("malformed Content-Length: " + contentLength.get(0), e);
      }
      long available = raw.length - bodyStart;
      if (length < 0 || available < length) {
        throw new IOException(
            "unexpected EOF: the body has " + available + " of " + length + " bytes");
      }
      return Arrays.copyOfRange(raw, bodyStart, bodyStart + (int) length);
    }
    return Arrays.copyOfRange(raw, bodyStart, raw.length);
  }

  private static byte[] dechunk(byte[] raw, int from) throws IOException {
    ByteArrayOutputStream body = new ByteArrayOutputStream();
    int pos = from;
    while (true) {
      int lineEnd = indexOf(raw, new byte[] {'\r', '\n'}, pos);
      if (lineEnd < 0) {
        throw new IOException("unexpected EOF in a chunk size line");
      }
      String sizeLine = new String(raw, pos, lineEnd - pos, StandardCharsets.ISO_8859_1);
      int semicolon = sizeLine.indexOf(';');
      String hex = (semicolon >= 0 ? sizeLine.substring(0, semicolon) : sizeLine).trim();
      int size;
      try {
        size = Integer.parseInt(hex, 16);
      } catch (NumberFormatException e) {
        throw new IOException("malformed chunk size: " + sizeLine, e);
      }
      pos = lineEnd + 2;
      if (size == 0) {
        // Trailers, if any, are not part of the body.
        return body.toByteArray();
      }
      if (size < 0 || raw.length - pos < size + 2) {
        throw new IOException("unexpected EOF inside a chunk of " + size + " bytes");
      }
      body.write(raw, pos, size);
      pos += size;
      if (raw[pos] != '\r' || raw[pos + 1] != '\n') {
        throw new IOException("malformed chunk: no CRLF after " + size + " bytes");
      }
      pos += 2;
    }
  }

  private static int indexOf(byte[] haystack, byte[] needle, int from) {
    outer:
    for (int i = from; i <= haystack.length - needle.length; i++) {
      for (int j = 0; j < needle.length; j++) {
        if (haystack[i + j] != needle[j]) {
          continue outer;
        }
      }
      return i;
    }
    return -1;
  }
}
