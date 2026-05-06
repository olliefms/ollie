# Dockerfile
FROM rust:1.91-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# Warm the dependency cache
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    echo 'pub fn lib(){}' > src/lib.rs && \
    cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ollie /usr/local/bin/ollie

RUN useradd -m -u 1001 ollie
RUN mkdir -p /data/blobs /data/lancedb && chown -R ollie:ollie /data
USER ollie

EXPOSE 3000
CMD ["ollie"]
