# minimart front end — one static artefact, no runtime server.
#
# Build context is examples/minimart-go-nuxt/ (see local/docker-compose.yml),
# so every path below is prefixed with frontend/:
#
#   docker build -f frontend/Dockerfile -t minimart-frontend:dev .
#
# `npm run generate` writes .output/public and then runs
# scripts/check-static-bundle.mjs, which fails the build if a hostname or an
# absolute API base got into the bundle. That check is the reason one prebuilt
# frontend container can be shared by every e2e scenario while each scenario
# runs on its own published port (PLAN.md §4): the bundle asks for
# /api/v1/... relative to whatever origin the browser loaded it from.
FROM node:20-alpine

WORKDIR /app

COPY frontend/package.json frontend/package-lock.json* ./
RUN npm install && npm install -g serve

# Copied file by file rather than as a whole directory: the build context is a
# developer's working tree, and `COPY frontend/ ./` would drag in node_modules
# built for the host's platform plus, later, the e2e suite in tests/ — which has
# no business inside the production image.
COPY frontend/nuxt.config.ts frontend/tsconfig.json ./
COPY frontend/app.vue ./
COPY frontend/assets ./assets
COPY frontend/composables ./composables
COPY frontend/i18n ./i18n
COPY frontend/layouts ./layouts
COPY frontend/pages ./pages
COPY frontend/plugins ./plugins
COPY frontend/public ./public
COPY frontend/scripts ./scripts
COPY frontend/types ./types
COPY frontend/utils ./utils

RUN npm run generate

EXPOSE 3000

# No --single. Every reachable route is prerendered to its own file
# (nuxt.config.ts, nitro.prerender.routes), and a catch-all rewrite would serve
# the root shell for all of them.
CMD ["serve", ".output/public", "--listen", "3000"]
