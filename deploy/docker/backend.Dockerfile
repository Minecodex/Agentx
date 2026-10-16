FROM rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder

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

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/agentx-service /usr/local/bin/agentx-service
COPY --from=builder /tmp/agentx-p3-fixture /usr/local/bin/agentx-p3-fixture
COPY --from=builder /workspace/src/plugins/packages/plugin-runner/runner.mjs /opt/agentx/plugin-runner/runner.mjs

ENV AGENTX_BIND_ADDR=0.0.0.0:8080
ENV AGENTX_PLUGIN_RUNNER_PATH=/opt/agentx/plugin-runner/runner.mjs
EXPOSE 8080 3128 3129 9091 9092

USER 65532:65532
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/agentx-service"]
