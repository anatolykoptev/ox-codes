# Stage 1: Chef
FROM rust:1.93-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# Stage 2: Planner
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin ox-codes

# Stage 4: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ox-codes /usr/local/bin/ox-codes

WORKDIR /app
ENV RUST_LOG=info
EXPOSE 8902

ENTRYPOINT ["ox-codes"]
CMD ["serve"]
