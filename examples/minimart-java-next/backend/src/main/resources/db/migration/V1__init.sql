CREATE TABLE users (
    id            SERIAL PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE,
    password_hash TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('guest', 'admin')),
    -- Deliberately no CHECK (points >= 0). The redemption transaction is the
    -- only writer and it refuses to go negative; the rule is asserted by the
    -- scenarios rather than absorbed silently by a constraint, so a bug in it
    -- shows up as a failing test instead of a rolled-back write nobody reads.
    points        INT     NOT NULL DEFAULT 0
);

CREATE TABLE products (
    id          SERIAL  PRIMARY KEY,
    name        TEXT    NOT NULL,
    cost_points INT     NOT NULL CHECK (cost_points > 0),
    -- -1 = unlimited, and deliberately no CHECK (stock >= -1): -1 is a
    -- sentinel, and the only writer is UPDATE ... WHERE stock > 0, which
    -- cannot produce any other negative value.
    stock       INT     NOT NULL,
    active      BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE orders (
    id          SERIAL      PRIMARY KEY,
    user_id     INT         NOT NULL REFERENCES users(id),
    product_id  INT         NOT NULL REFERENCES products(id),
    -- Copied from the product at redemption time on purpose: this is the price
    -- paid, not the price now. Changing products.cost_points must not rewrite
    -- history.
    cost_points INT         NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_orders_user_id ON orders (user_id, id DESC);
CREATE INDEX idx_products_active ON products (active);
