# syntax=docker/dockerfile:1
# The builder MUST be on the same Debian release as the runtime stage below.
# The unsuffixed rust:*-slim tag silently aliases the newest Debian (trixie,
# glibc 2.41); a binary linked there can require glibc symbols the bookworm
# runtime (glibc 2.36) doesn't have — v2.5.0 crash-looped on GLIBC_2.39
# (pidfd_spawn, pulled in by the tesseract subprocess spawn). Keep the
# -bookworm suffix in lockstep with the runtime FROM, or move both at once.
FROM rust:1.91-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Cache mounts persist the cargo registry and target dir across builds, even
# when this layer is invalidated (e.g. by a version bump touching Cargo.toml).
# Only crates that actually changed recompile — mirroring local incremental
# builds. The binary is copied out of the cache mount since mounts aren't
# captured in the image layer.
# The target cache is keyed to the builder's Debian release: it holds compiled
# build scripts that the builder must be able to execute, and artifacts from a
# newer-glibc base crash with "GLIBC_x.yy not found" on an older one. Bump the
# id suffix whenever the builder base image's Debian release changes.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-bookworm,target=/app/target \
    cargo build --release && cp target/release/ollie /usr/local/bin/ollie

FROM debian:bookworm-slim

# tesseract-ocr powers the scanned-document OCR path in the AI pipeline —
# without it, scans degrade to the (unreliable) vision model. (#372)
RUN apt-get update && apt-get install -y ca-certificates libssl3 tesseract-ocr tesseract-ocr-eng && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /usr/local/bin/ollie /usr/local/bin/ollie
COPY static ./static

RUN useradd -m -u 1001 ollie
RUN mkdir -p /data/blobs /data/lancedb && chown -R ollie:ollie /data /app
USER ollie

EXPOSE 3000
CMD ["ollie"]
