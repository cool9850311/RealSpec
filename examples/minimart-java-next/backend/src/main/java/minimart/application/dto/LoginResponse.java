package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/**
 * The 200 body of POST /api/v1/auth/login. It carries neither the password hash nor the token — the
 * token is only ever a cookie.
 */
@JsonPropertyOrder({"username", "role"})
public record LoginResponse(String username, String role) {}
