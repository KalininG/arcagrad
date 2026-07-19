# Keep every Rust stage on the same glibc base so cargo-chef artifacts remain compatible.

FROM node:20-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1-trixie AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
        libvips-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
RUN rustup target add wasm32-unknown-unknown
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml ./
COPY Cargo.lock* ./
COPY build.rs ./
COPY src ./src
COPY plugin-sdk ./plugin-sdk
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Copy application sources only after the cached dependency build.
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Migrations, the SPA, and plugins are embedded at compile time.
COPY Cargo.toml ./
COPY Cargo.lock* ./
COPY build.rs ./
COPY src ./src
COPY plugin-sdk ./plugin-sdk
COPY migrations ./migrations
COPY plugins ./plugins
COPY --from=web /web/build ./web/build
RUN cargo build --release

FROM debian:trixie-slim AS runtime

# Older Debian bases still use the non-t64 libvips package name.
RUN apt-get update \
    && ( apt-get install -y --no-install-recommends libvips42t64 \
         || apt-get install -y --no-install-recommends libvips42 ) \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 1000 -m -s /usr/sbin/nologin arca

COPY --from=builder /app/target/release/arcagrad /usr/local/bin/arcagrad

ENV ARCA_CONTENT_DIR=/content \
    ARCA_DATA_DIR=/data \
    ARCA_BIND=0.0.0.0:3000 \
    RUST_LOG=info,tower_http=info \
    # Limit allocator fragmentation on long-running threaded servers.
    MALLOC_ARENA_MAX=2

RUN mkdir -p /content /data

# Runs as root so newly created bind mounts are writable. Operators may supply --user.
EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/arcagrad", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/arcagrad"]
