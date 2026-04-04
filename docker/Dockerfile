# Stage 1: Build
FROM rust:1.86-slim AS builder

WORKDIR /app

# Cache dependency builds by copying manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src target/release/deps/Rusty_Data_Capture*

# Build the actual application
COPY src/ src/
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/Rusty-Data-Capture /usr/local/bin/solar-collector

ENTRYPOINT ["solar-collector"]
