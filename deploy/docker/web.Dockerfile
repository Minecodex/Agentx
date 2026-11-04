FROM node:24-alpine@sha256:8a32c8cf649a49915a66e81e7a609223c708d0adabf288ee3f4077d765542f85 AS builder

WORKDIR /workspace
RUN corepack enable

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY src/web/package.json ./src/web/package.json
COPY tests/browser/package.json ./tests/browser/package.json
COPY src/plugins/packages/plugin-sdk/package.json ./src/plugins/packages/plugin-sdk/package.json
COPY src/plugins/packages/plugin-ui/package.json ./src/plugins/packages/plugin-ui/package.json
COPY src/plugins/packages/plugin-runner/package.json ./src/plugins/packages/plugin-runner/package.json
COPY src/plugins/builtin/data/package.json ./src/plugins/builtin/data/package.json
COPY src/plugins/builtin/http/package.json ./src/plugins/builtin/http/package.json
COPY src/plugins/templates/canvas-plugin/package.json ./src/plugins/templates/canvas-plugin/package.json
COPY src/plugins/templates/canvas-plugin/vendor ./src/plugins/templates/canvas-plugin/vendor
RUN --mount=type=cache,id=agentx-pnpm-store,target=/root/.local/share/pnpm/store,sharing=locked \
    pnpm install --frozen-lockfile --fetch-timeout=300000

COPY src/web ./src/web
COPY contracts/sdk ./contracts/sdk
RUN pnpm build:web \
    && touch /workspace/src/web/dist/runtime-config.js

FROM nginxinc/nginx-unprivileged:1.28-alpine@sha256:7377697a821c131a924a7105fafbe7414db4e9fcc77a6f08f776f33f141ec3f8 AS runtime

ARG NGINX_CONFIG=deploy/docker/nginx.conf
COPY ${NGINX_CONFIG} /etc/nginx/conf.d/default.conf
COPY --chown=101:101 --from=builder /workspace/src/web/dist /opt/agentx-web
COPY --chmod=755 deploy/docker/web-entrypoint.d/40-runtime-config.sh /docker-entrypoint.d/40-runtime-config.sh

EXPOSE 8080
