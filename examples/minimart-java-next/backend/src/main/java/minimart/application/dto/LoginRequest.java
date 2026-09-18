package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/**
 * The body of POST /api/v1/auth/login. Both fields are nullable here on purpose: absent, null and
 * empty are all the same 400, and it is the controller that says so, not a default value quietly
 * standing in for a missing field.
 */
@JsonPropertyOrder({"username", "password"})
public record LoginRequest(String username, String password) {

  @Override
  public String toString() {
    // A request body is the kind of value that ends up in a log line; the password must not.
    return "LoginRequest[username=" + username + "]";
  }
}
