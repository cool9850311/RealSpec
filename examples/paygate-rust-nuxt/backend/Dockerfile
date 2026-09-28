# One image, four binaries. The container picks which one by its command, so a
# scenario's eleven containers are eleven starts of one build rather than eleven
# builds (spec.md, "Isolation").
#
# Build context is the example root: docker build -f backend/Dockerfile .

FROM rust:1.93-bookworm AS builder

# librdkafka is built from source by rdkafka's `cmake-build` feature.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake g++ libssl-dev pkg-config zlib1g-dev \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Dependencies first, so that editing a source file does not recompile the world.
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/crates/domain/Cargo.toml        crates/domain/Cargo.toml
COPY backend/crates/provider/Cargo.toml      crates/provider/Cargo.toml
COPY backend/crates/infra/Cargo.toml         crates/infra/Cargo.toml
COPY backend/crates/kafkabus/Cargo.toml      crates/kafkabus/Cargo.toml
COPY backend/crates/api/Cargo.toml           crates/api/Cargo.toml
COPY backend/crates/worker/Cargo.toml        crates/worker/Cargo.toml
COPY backend/crates/provider-mock/Cargo.toml crates/provider-mock/Cargo.toml
COPY backend/crates/demo-merchant/Cargo.toml crates/demo-merchant/Cargo.toml
RUN mkdir -p crates/domain/src crates/provider/src crates/infra/src crates/kafkabus/src \
             crates/api/src crates/worker/src crates/provider-mock/src crates/demo-merchant/src \
 && echo '' > crates/domain/src/lib.rs \
 && echo '' > crates/provider/src/lib.rs \
 && echo '' > crates/infra/src/lib.rs \
 && echo '' > crates/kafkabus/src/lib.rs \
 && for c in api worker provider-mock demo-merchant; do echo 'fn main() {}' > "crates/$c/src/main.rs"; done \
 && cargo build --release --locked \
 && rm -rf crates/*/src

COPY backend/crates ./crates
# The migrations are needed at COMPILE time, not just at run time: `infra` embeds
# them with sqlx's `migrate!` macro, which reads the directory while the crate is
# being compiled. Without this the build fails with "error canonicalizing
# migration directory", and the runtime COPY below is too late to help.
COPY backend/migrations ./migrations

# cargo does not rebuild a crate whose mtime is older than the stub it replaced.
RUN find crates -name '*.rs' -exec touch {} + \
 && cargo build --release --locked \
 && strip target/release/paygate-api \
          target/release/paygate-worker \
          target/release/paygate-provider-mock \
          target/release/paygate-demo-merchant

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl tzdata \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /app paygate

WORKDIR /app
COPY --from=builder /build/target/release/paygate-api            /usr/local/bin/
COPY --from=builder /build/target/release/paygate-worker         /usr/local/bin/
COPY --from=builder /build/target/release/paygate-provider-mock  /usr/local/bin/
COPY --from=builder /build/target/release/paygate-demo-merchant  /usr/local/bin/
COPY backend/migrations /app/migrations

ENV MIGRATIONS_DIR=/app/migrations \
    PORT=8080 \
    LOG_LEVEL=info
EXPOSE 8080
USER paygate

# No HEALTHCHECK here on purpose: the four binaries have four different
# readiness paths, each one a route that already exists in its own spec
# (`/api/v1/health/live` for the API, the worker's admin `/healthz`, the mock's
# `/provider/__control/requests`, the merchant's `/demo-merchant/api/__deliveries`).
# The stack states the wait strategy per container, which is where it belongs.

CMD ["paygate-api"]
