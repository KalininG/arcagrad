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
    && apt-get install -y --no-install-recommends libjemalloc2 \
    && rm -rf /var/lib/apt/lists/* \
    # One fixed path for LD_PRELOAD across amd64 and arm64.
    && ln -s /usr/lib/*-linux-gnu/libjemalloc.so.2 /usr/local/lib/libjemalloc.so.2 \
    && test -e /usr/local/lib/libjemalloc.so.2 \
    && useradd -u 1000 -m -s /usr/sbin/nologin arca

COPY --from=builder /app/target/release/arcagrad /usr/local/bin/arcagrad

ENV ARCA_CONTENT_DIR=/content \
    ARCA_DATA_DIR=/data \
    ARCA_BIND=0.0.0.0:3000 \
    # jemalloc returns freed memory after scans and busy periods; glibc keeps it.
    LD_PRELOAD=/usr/local/lib/libjemalloc.so.2 \
    # Without the background thread jemalloc only purges on allocation, so an idle server keeps it.
    # Huge pages can't be returned while partly in use, so opt out on hosts with THP set to always.
    MALLOC_CONF=background_thread:true,thp:never

RUN mkdir -p /content /data

# Runs as root so newly created bind mounts are writable. Operators may supply --user.
EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/arcagrad", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/arcagrad"]
