package minimart.application.dto;

import com.fasterxml.jackson.annotation.JsonPropertyOrder;

/** The 200 body of GET /api/v1/me. */
@JsonPropertyOrder({"username", "role", "points"})
public record MeResponse(String username, String role, int points) {}
