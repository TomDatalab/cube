# syntax=docker/dockerfile:1.7
#
# Cube with the Rust backend: one `cube-server` binary serving the REST,
# GraphQL, WebSocket and SQL APIs, plus the Playground as static files.
# No Node.js in the image.
#
# Build from the repository root (the context must be the root, because the
# workspace depends on rust/cubesql and the Playground lives in packages/):
#
#   docker buildx build -f rust/cube/docker/Dockerfile \
#     --platform linux/amd64,linux/arm64 -t sofiadb/cube-rust:dev .
#
# The Playground is copied pre-built (see README.md, "Building"): build it
# once with `yarn build` in packages/cubejs-playground and its vizard/.
#
# arm64 is cross-compiled on the build machine instead of emulated, and the
# runtime stage has no RUN step, so no QEMU is needed for either platform.

ARG RUST_IMAGE=rust:1-bookworm
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot

# ------------------------------------------------------------------ cube-server
FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS server
ARG TARGETARCH

RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang pkg-config \
 && if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross; \
    fi \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY rust/cubesql rust/cubesql
COPY rust/cube rust/cube

# The runtime image has libstdc++ (DuckDB) but no zlib: link it statically.
ENV LIBZ_SYS_STATIC=1

RUN --mount=type=cache,id=cube-rust-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cube-rust-target-${TARGETARCH},target=/src/rust/cube/target \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) TRIPLE=x86_64-unknown-linux-gnu ;; \
      arm64) TRIPLE=aarch64-unknown-linux-gnu; \
             export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
                    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
                    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
                    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc ;; \
      *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    cd rust/cube; \
    rustup target add "$TRIPLE"; \
    cargo build --release --locked -p cubeserver --target "$TRIPLE"; \
    cp "target/$TRIPLE/release/cube-server" /cube-server

# ------------------------------------------------------------------- Playground
FROM --platform=$BUILDPLATFORM python:3.12-slim AS playground
WORKDIR /work
COPY packages/cubejs-playground/build playground
COPY packages/cubejs-playground/vizard/build playground/vizard
COPY rust/cube/docker/patch-playground.py .
RUN python3 patch-playground.py playground

# ---------------------------------------------------------------------- runtime
FROM ${RUNTIME_IMAGE}

LABEL org.opencontainers.image.title="cube-rust" \
      org.opencontainers.image.description="Community build of Cube with a Rust backend (no Node.js). Not affiliated with or endorsed by Cube Dev, Inc." \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.vendor="sofiadb"

COPY --from=server /cube-server /usr/local/bin/cube-server
COPY --from=playground /work/playground /cube/playground
COPY LICENSE /licenses/LICENSE

# Same layout as the official image: mount the project (model/, cube.yml)
# at /cube/conf.
WORKDIR /cube/conf
ENV PORT=4000 \
    CUBEJS_SCHEMA_PATH=model \
    CUBEJS_PLAYGROUND_PATH=/cube/playground

# 4000: REST, GraphQL, WebSocket, Playground. 15432: SQL API, when
# CUBEJS_PG_SQL_PORT=15432 is set.
EXPOSE 4000 15432

USER nonroot
ENTRYPOINT ["/usr/local/bin/cube-server"]
