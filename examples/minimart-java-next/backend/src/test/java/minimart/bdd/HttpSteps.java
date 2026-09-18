package minimart.bdd;

import io.cucumber.docstring.DocString;
import io.cucumber.java.en.When;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;

/**
 * The two request steps of spec/bdd/format.yml: http_request and http_request_concurrent.
 *
 * <p>Neither keeps a cookie jar, so a request sends exactly the headers its own docstring writes. A
 * request that is made as somebody says so itself, with {@code "Cookie": "token={someVar}"} — the
 * whole docstring goes through {@link ScenarioContext#resolve}, so a token saved by {@code save
 * response cookie}, and a pre-minted one, are substituted there like anything else in the bag.
 *
 * <p>Transport errors fail the step. Any HTTP status, 5xx included, is recorded and left to the
 * assertion steps.
 */
public final class HttpSteps {

  private final ScenarioContext context;

  public HttpSteps(ScenarioContext context) {
    this.context = context;
  }

  /** {@code <METHOD> <path>:} — one request, recorded as the response the assertions read. */
  @When("^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+):$")
  public void httpRequest(String method, String rawPath, DocString doc) {
    String path = context.resolve(rawPath);
    RequestEnvelope envelope = RequestEnvelope.parseSingle(context.resolve(doc.getContent()));

    HttpRequest request;
    try {
      HttpRequest.Builder builder =
          HttpRequest.newBuilder(URI.create(context.baseUrl() + path))
              .timeout(ScenarioContext.REQUEST_TIMEOUT)
              .method(
                  method,
                  envelope.body() == null
                      ? HttpRequest.BodyPublishers.noBody()
                      : HttpRequest.BodyPublishers.ofString(
                          envelope.body(), StandardCharsets.UTF_8));
      envelope.effectiveHeaders().forEach(builder::setHeader);
      request = builder.build();
    } catch (IllegalArgumentException e) {
      throw new AssertionError("build request: " + e.getMessage(), e);
    }

    HttpResponse<byte[]> response;
    try {
      response = context.httpClient().send(request, HttpResponse.BodyHandlers.ofByteArray());
    } catch (IOException e) {
      throw new AssertionError(method + " " + path + " failed: " + e, e);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new AssertionError(method + " " + path + " interrupted", e);
    }
    // One response is on record again; the concurrent set is not.
    context.recordResponse(
        new ScenarioContext.Response(
            response.statusCode(), response.headers().map(), response.body()));
  }

  /**
   * {@code <METHOD> <path> is called concurrently:} — every caller at one instant, recorded as a
   * set.
   *
   * <p>Every request is serialised and every connection opened before any caller is released, and
   * the request is written onto that connection rather than handed to an HTTP client: the gate
   * opens onto a write and nothing else. The race window is the winner's transaction, under a
   * millisecond against a tmpfs database, so a caller still shaking hands with Docker's published
   * port arrives after it has closed. The raw write costs nothing — the scenario's client exists to
   * keep no cookie jar and to refuse redirects, which a bare connection does by having neither.
   *
   * <p>The other half is the feature's: the service takes a database connection per in-flight
   * request, and one being opened is one not racing, so a scenario warms the pool with a concurrent
   * read first.
   *
   * <p>The outcome is a SET and the single response is cleared, so an assertion that names one
   * fails through {@link ScenarioContext#requireResponse()} rather than reading a stale answer.
   */
  @When("^(GET|POST|PUT|PATCH|DELETE) (/api/v1/[a-zA-Z0-9/{}:._?=&%-]+) is called concurrently:$")
  public void httpRequestConcurrent(String method, String rawPath, DocString doc)
      throws InterruptedException {
    String path = context.resolve(rawPath);
    List<RequestEnvelope> envelopes = RequestEnvelope.parseArray(context.resolve(doc.getContent()));
    int callers = envelopes.size();
    if (callers < 2) {
      throw new AssertionError(
          "concurrent request docstring lists " + callers + " caller(s); a race needs at least 2");
    }

    context.clearResponses();

    URI base = URI.create(context.baseUrl());
    InetSocketAddress address = new InetSocketAddress(base.getHost(), base.getPort());
    String authority = base.getRawAuthority();
    int timeoutMillis = (int) ScenarioContext.REQUEST_TIMEOUT.toMillis();

    byte[][] requests = new byte[callers][];
    Socket[] sockets = new Socket[callers];
    long[] deadlines = new long[callers];
    try {
      for (int i = 0; i < callers; i++) {
        RequestEnvelope envelope = envelopes.get(i);
        requests[i] =
            RawHttp.serialize(
                method, path, authority, envelope.effectiveHeaders(), envelope.body());
        Socket socket = new Socket();
        sockets[i] = socket;
        try {
          socket.connect(address, timeoutMillis);
        } catch (IOException e) {
          throw new AssertionError(
              caller(i, callers) + "connect to " + authority + ": " + e.getMessage(), e);
        }
        // The deadline starts at connect, as go-nuxt's SetDeadline does: one bound on everything
        // this caller does from here on.
        deadlines[i] = System.nanoTime() + ScenarioContext.REQUEST_TIMEOUT.toNanos();
      }

      ScenarioContext.RecordedResponse[] responses = new ScenarioContext.RecordedResponse[callers];
      Throwable[] errors = new Throwable[callers];

      // ready counts the callers parked at the gate; gate is the gate. Counting it down once
      // releases every waiter at the same instant.
      CountDownLatch ready = new CountDownLatch(callers);
      CountDownLatch gate = new CountDownLatch(1);
      List<Thread> threads = new ArrayList<>(callers);
      for (int i = 0; i < callers; i++) {
        final int caller = i;
        threads.add(
            Thread.ofPlatform()
                .name("concurrent-caller-" + (caller + 1))
                .start(
                    () -> {
                      ready.countDown();
                      try {
                        gate.await();
                        responses[caller] =
                            exchange(sockets[caller], requests[caller], deadlines[caller]);
                      } catch (Throwable t) {
                        errors[caller] = t;
                      }
                    }));
      }

      ready.await();
      gate.countDown();
      for (Thread thread : threads) {
        thread.join();
      }

      for (int i = 0; i < callers; i++) {
        if (errors[i] != null) {
          throw new AssertionError(caller(i, callers) + errors[i].getMessage(), errors[i]);
        }
      }
      context.recordResponseSet(List.of(responses));
    } finally {
      for (Socket socket : sockets) {
        if (socket != null) {
          try {
            socket.close();
          } catch (IOException e) {
            // Nothing is left to read from it; a close that fails changes no result.
          }
        }
      }
    }
  }

  private static ScenarioContext.RecordedResponse exchange(
      Socket socket, byte[] request, long deadline) throws IOException {
    try {
      OutputStream out = socket.getOutputStream();
      out.write(request);
      out.flush();
    } catch (IOException e) {
      throw new IOException("write request: " + e.getMessage(), e);
    }
    try {
      return RawHttp.parse(RawHttp.readToEof(socket, deadline));
    } catch (IOException e) {
      throw new IOException("read response: " + e.getMessage(), e);
    }
  }

  private static String caller(int index, int of) {
    return "caller " + (index + 1) + " of " + of + ": ";
  }
}
