# syntax=docker/dockerfile:1.4

# Stage 1: Chef
FROM rust:1.93-bookworm AS chef
RUN apt-get update && apt-get install -y --no-install-recommends clang mold curl && rm -rf /var/lib/apt/lists/*
# sccache: content-addressed compiler cache — hits survive BuildKit cache
# invalidation on source changes; mold replaces gold linker (3-5x faster link).
ENV SCCACHE_VERSION=0.10.0
RUN ARCH=$(uname -m) && \
    curl -fsSL "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-${ARCH}-unknown-linux-musl.tar.gz" \
    | tar xz --strip-components=1 -C /usr/local/bin "sccache-v${SCCACHE_VERSION}-${ARCH}-unknown-linux-musl/sccache" && \
    chmod +x /usr/local/bin/sccache
RUN cargo install cargo-chef --locked
ENV RUSTFLAGS="-C linker=clang -C link-arg=-fuse-ld=mold"
WORKDIR /app

# Stage 2: Planner
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder
FROM chef AS builder
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/sccache
ENV SCCACHE_CACHE_SIZE=20G
ENV SCCACHE_IDLE_TIMEOUT=0
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=clang
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold"
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold"
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/sccache \
    cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/sccache \
    cargo build --release --locked --bin ox-codes && \
    cp target/release/ox-codes /binary && \
    sccache --show-stats || true

# Stage 4: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /binary /usr/local/bin/ox-codes

WORKDIR /app
ENV RUST_LOG=info
EXPOSE 8902

ENTRYPOINT ["ox-codes"]
CMD ["serve"]
