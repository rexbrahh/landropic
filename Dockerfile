# Multi-stage build for minimal image size
FROM rust:1.75 AS builder

WORKDIR /usr/src/landropic

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY landro-cas/Cargo.toml landro-cas/Cargo.toml
COPY landro-index/Cargo.toml landro-index/Cargo.toml
COPY landro-chunker/Cargo.toml landro-chunker/Cargo.toml
COPY landro-crypto/Cargo.toml landro-crypto/Cargo.toml
COPY landro-proto/Cargo.toml landro-proto/Cargo.toml
COPY landro-quic/Cargo.toml landro-quic/Cargo.toml
COPY landro-sync/Cargo.toml landro-sync/Cargo.toml
COPY landro-daemon/Cargo.toml landro-daemon/Cargo.toml
COPY landro-cli/Cargo.toml landro-cli/Cargo.toml

# Create dummy files to cache dependencies
RUN mkdir -p landro-{cas,index,chunker,crypto,proto,quic,sync,daemon,cli}/src && \
    echo "fn main() {}" > landro-daemon/src/main.rs && \
    echo "fn main() {}" > landro-cli/src/main.rs && \
    echo "// dummy" > landro-cas/src/lib.rs && \
    echo "// dummy" > landro-index/src/lib.rs && \
    echo "// dummy" > landro-chunker/src/lib.rs && \
    echo "// dummy" > landro-crypto/src/lib.rs && \
    echo "// dummy" > landro-proto/src/lib.rs && \
    echo "// dummy" > landro-quic/src/lib.rs && \
    echo "// dummy" > landro-sync/src/lib.rs

# Build dependencies
RUN cargo build --release --bin landro-daemon --bin landro-cli

# Copy actual source code
COPY . .

# Touch main.rs to trigger rebuild with actual code
RUN touch landro-daemon/src/main.rs landro-cli/src/main.rs && \
    cargo build --release --bin landro-daemon --bin landro-cli

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/landropic/target/release/landro-daemon /usr/local/bin/
COPY --from=builder /usr/src/landropic/target/release/landro-cli /usr/local/bin/

# Create data directory
RUN mkdir -p /data && \
    useradd -m -u 1000 landropic && \
    chown -R landropic:landropic /data

USER landropic
WORKDIR /data

# QUIC port
EXPOSE 9990/tcp 9990/udp

# mDNS port
EXPOSE 5353/udp

VOLUME ["/data"]

ENTRYPOINT ["landro-daemon"]
CMD ["--config", "/data/config.toml"]