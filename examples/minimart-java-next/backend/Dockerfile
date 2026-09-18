# minimart backend.
#
# The build context is examples/minimart-java-next (the parent of this file's
# directory), because local/docker-compose.yml builds it as
# `context: ..`, `dockerfile: backend/Dockerfile` — and the e2e stack and the
# Cucumber-JVM suite build the same two inputs the same way
# (`docker build -f backend/Dockerfile .`), so every environment runs one image
# built one way.
#
# The result is the Spring Boot executable jar on a JRE: nothing in the runtime
# image but the Java runtime and the service. The migrations are inside the jar
# (src/main/resources/db/migration), so unlike the Go image there is no
# directory of SQL files to copy beside it.

FROM eclipse-temurin:25-jdk AS builder
WORKDIR /src

# The wrapper and the POM first, and every dependency and plugin resolved
# against them, so a source-only change does not re-download the world. The
# wrapper is distributionType=source: this image has no curl or wget, so the
# wrapper jar is fetched by a small Java downloader instead.
COPY backend/mvnw backend/pom.xml ./
COPY backend/.mvn .mvn
RUN ./mvnw -B -q dependency:go-offline

COPY backend/src src
# Tests are not run here: the unit tests run in `./mvnw verify` in CI, and the
# API suite is what builds this very image. They are still compiled, under the
# same -Xlint:all -Werror as the service.
RUN ./mvnw -B -DskipTests package

FROM eclipse-temurin:25-jre-alpine
WORKDIR /app
COPY --from=builder /src/target/minimart.jar /app/minimart.jar

EXPOSE 8080
USER 10001:10001
# ExitOnOutOfMemoryError: a JVM that has run out of heap is not a service that
# can recover; exiting lets the orchestrator restart it instead of leaving it
# half-alive and answering 500s.
ENTRYPOINT ["java", "-XX:+ExitOnOutOfMemoryError", "-jar", "/app/minimart.jar"]
