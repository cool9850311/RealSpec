package minimart.infrastructure.repository;

import java.util.Optional;
import minimart.domain.User;
import org.springframework.jdbc.core.RowMapper;
import org.springframework.jdbc.core.simple.JdbcClient;

/**
 * Reads the users table. The only writer of users.points is the redemption transaction in {@link
 * OrderRepository}.
 */
public class UserRepository {

  private static final RowMapper<User> USER =
      (rs, rowNum) ->
          new User(
              rs.getInt("id"),
              rs.getString("username"),
              rs.getString("password_hash"),
              rs.getString("role"),
              rs.getInt("points"));

  private final JdbcClient jdbc;

  /** A repository backed by {@code jdbc}. */
  public UserRepository(JdbcClient jdbc) {
    this.jdbc = jdbc;
  }

  /** Returns the user with the given username, or empty. */
  public Optional<User> findByUsername(String username) {
    return jdbc.sql(
            "SELECT id, username, password_hash, role, points FROM users WHERE username = ?")
        .param(username)
        .query(USER)
        .optional();
  }

  /**
   * Returns just the role of the user with the given id, or empty.
   *
   * <p>It is a query of its own rather than a {@link #findById} whose other columns are discarded,
   * because it runs on every request to a role-guarded endpoint: the password hash of the caller
   * has no business being read, let alone carried through the process, to answer a question about
   * authority.
   */
  public Optional<String> findRoleById(long id) {
    return jdbc.sql("SELECT role FROM users WHERE id = ?").param(id).query(String.class).optional();
  }

  /** Returns the user with the given id, or empty. */
  public Optional<User> findById(long id) {
    return jdbc.sql("SELECT id, username, password_hash, role, points FROM users WHERE id = ?")
        .param(id)
        .query(USER)
        .optional();
  }
}
