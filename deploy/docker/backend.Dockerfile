FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS toolchain

FROM node:24-alpine@sha256:8a32c8cf649a49915a66e81e7a609223c708d0adabf288ee3f4077d765542f85 AS design-preview
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
RUN pnpm --filter @agentx/web build:design-preview

FROM toolchain AS builder

ARG APP
ARG CARGO_PACKAGE
WORKDIR /workspace

# The base image already pins the compiler. Omitting rust-toolchain.toml avoids
# rustup downloading development-only components for every service image.
COPY Cargo.toml Cargo.lock ./
COPY src/crates ./src/crates
COPY src/services ./src/services
COPY src/plugins ./src/plugins
COPY tests/fixtures ./tests/fixtures
COPY tools ./tools
COPY deploy ./deploy
COPY contracts ./contracts

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target \
    find src/crates src/services tools tests/fixtures -type f -name '*.rs' -exec touch {} + \
    && if [ -n "$CARGO_PACKAGE" ]; then \
        CARGO_TARGET_DIR="/workspace/target/$CARGO_PACKAGE" \
            cargo build --release --locked --package "$CARGO_PACKAGE" --bin "$APP" \
        && cp "/workspace/target/$CARGO_PACKAGE/release/$APP" /tmp/agentx-service; \
    else \
        cargo build --release --locked --bin "$APP" \
        && cp "target/release/$APP" /tmp/agentx-service; \
    fi \
    && if [ "$APP" = "platform-control" ]; then \
        cargo build --release --locked --package platform-control --bin v2-04-fixture \
        && cp target/release/v2-04-fixture /tmp/agentx-p3-fixture; \
    else \
        cp /bin/false /tmp/agentx-p3-fixture; \
    fi

FROM node:24.20.0-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS runtime

COPY --from=toolchain /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/agentx-service /usr/local/bin/agentx-service
COPY --from=builder /tmp/agentx-p3-fixture /usr/local/bin/agentx-p3-fixture
COPY --from=builder /workspace/src/plugins/packages/plugin-runner/runner.mjs /opt/agentx/plugin-runner/runner.mjs
COPY --from=design-preview /workspace/.local/dist/design-preview /opt/agentx/design-preview

ENV AGENTX_BIND_ADDR=0.0.0.0:8080
ENV AGENTX_PLUGIN_RUNNER_PATH=/opt/agentx/plugin-runner/runner.mjs
EXPOSE 8080 3128 3129 9091 9092

USER 65532:65532
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/agentx-service"]
