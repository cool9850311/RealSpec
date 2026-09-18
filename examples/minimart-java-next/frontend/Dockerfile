# minimart front end — one static artefact, no runtime server.
#
# Build context is examples/minimart-java-next/ (see local/docker-compose.yml),
# so every path below is prefixed with frontend/:
#
#   docker build -f frontend/Dockerfile -t minimart-next-frontend:dev .
#
# `npm run build` runs `next build` (output: 'export', see next.config.ts) and
# then scripts/check-static-bundle.mjs, which fails the build if the export is
# incomplete or an absolute API address got into the bundle. That check is the
# reason one prebuilt frontend container can be shared by every e2e scenario
# while each scenario runs on its own published port: the bundle asks for
# /api/v1/... relative to whatever origin the browser loaded it from.
FROM node:24-alpine

WORKDIR /app

# The build is one `next build`; nobody reads its telemetry.
ENV NEXT_TELEMETRY_DISABLED=1

# package.json alone, with no lockfile beside it: this directory is a workspace
# of the repository-root package.json, so its lock lives in the root
# package-lock.json, which is outside this build context (the context is the
# example, so that the image cannot depend on anything else in the repository).
# `npm install` therefore resolves within the ranges package.json declares. (The
# go-nuxt front end copies a frontend-local package-lock.json at this step; npm
# never writes one for a workspace, so this front end has none to copy.)
COPY frontend/package.json ./
RUN npm install && npm install -g serve

# Copied file by file rather than as a whole directory: the build context is a
# developer's working tree, and `COPY frontend/ ./` would drag in node_modules
# built for the host's platform, a stale .next/ or out/, and the e2e suite in
# tests/ — which has no business inside the production image.
COPY frontend/next.config.ts frontend/tsconfig.json frontend/global.d.ts ./
COPY frontend/app ./app
COPY frontend/components ./components
COPY frontend/i18n ./i18n
COPY frontend/lib ./lib
COPY frontend/public ./public
COPY frontend/scripts ./scripts

RUN npm run build

EXPOSE 3000

# No --single. Every reachable route is exported to its own file (login.html,
# products.html, orders.html), `serve`'s default clean URLs map /login onto
# login.html, and a catch-all rewrite would serve the root document for all of
# them — including the RSC payload files next/link prefetches, which would then
# answer 200 with HTML instead of 404 and fail somewhere much harder to read.
CMD ["serve", "out", "--listen", "3000"]
