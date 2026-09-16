# minimart backend.
#
# The build context is examples/minimart-go-nuxt (the parent of this file's
# directory), because local/docker-compose.yml builds it as
# `context: ..`, `dockerfile: backend/Dockerfile` — and the e2e stack and the
# godog suite build the same two inputs the same way, so every environment runs
# one image built one way.
#
# The result is a static binary on `scratch`: nothing in the runtime image but
# the service and the migrations it may have to apply.

FROM golang:1.24-alpine AS builder
WORKDIR /src

# Dependencies first, so a source-only change does not re-download them.
COPY backend/go.mod backend/go.sum ./
RUN go mod download

COPY backend/ ./
# CGO off and a trimmed path: the binary must run on an image with no libc and
# must not embed this machine's directory layout.
RUN CGO_ENABLED=0 GOOS=linux go build -trimpath -ldflags="-s -w" -o /out/minimart ./cmd

FROM scratch
WORKDIR /app
COPY --from=builder /out/minimart /app/minimart
# SCHEMA_AUTO_MIGRATE reads these; config.MigrationsDir is "migrations",
# relative to WORKDIR.
COPY backend/migrations /app/migrations

EXPOSE 8080
USER 10001:10001
ENTRYPOINT ["/app/minimart"]
