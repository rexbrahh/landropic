# Landropic

**Cross-platform encrypted LAN file sync - AirDrop for everyone**

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org) [![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://github.com/landropic/landropic#license)

Landropic is a secure, fast, and reliable file synchronization tool designed for local networks. It provides end-to-end encryption, automatic device discovery, and efficient chunked file transfer using modern protocols.

## Features

- **🔒 End-to-End Encryption**: Files are encrypted using ChaCha20-Poly1305 with Ed25519 device authentication
- **🚀 High Performance**: QUIC-based transport with content-defined chunking for optimal throughput
- **🔍 Auto Discovery**: Automatic device discovery using mDNS/Zeroconf
- **📁 Real-time Sync**: Continuous file watching and synchronization
- **⚡ Resume Support**: Interrupted transfers automatically resume from where they left off
- **🖥️ Cross Platform**: Works on Linux, macOS, and Windows
- **📊 Progress Tracking**: Real-time sync progress with detailed status reporting

## Quick Start

### Installation

```bash
# Clone the repository
git clone https://github.com/landropic/landropic.git
cd landropic

# Build the project
cargo build --release

# Install binaries
cargo install --path .
```

### Basic Usage

1. **Initialize Landropic on your device:**
   ```bash
   landropic init --name "My Device"
   ```

2. **Pair with another device:**
   ```bash
   # On first device - show pairing QR code
   landropic pair --show-qr

   # On second device - scan QR or enter code
   landropic pair --code "ABC123..."
   ```

3. **Start syncing a folder:**
   ```bash
   # Sync once
   landropic sync ~/Documents/MyFolder

   # Watch for continuous changes
   landropic sync ~/Documents/MyFolder --watch
   ```

4. **Check sync status:**
   ```bash
   landropic status
   ```

5. **List paired devices:**
   ```bash
   landropic peers
   ```

### Daemon Management

Landropic runs a background daemon to handle continuous synchronization:

```bash
# Start the daemon
landropic daemon start

# Check daemon status
landropic daemon status

# Stop the daemon
landropic daemon stop
```

## Architecture Overview

Landropic uses a modular architecture with clear separation of concerns:

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   CLI Tool  │    │   Daemon    │    │ Peer Device │
│             │    │             │    │             │
├─────────────┤    ├─────────────┤    ├─────────────┤
│ Commands    │───▶│ Orchestrator│◀──▶│ QUIC Server │
│ Progress UI │    │ Sync Engine │    │ mTLS Auth   │
│ Config Mgmt │    │ File Watcher│    │ Protocol    │
└─────────────┘    │ mDNS Discov │    └─────────────┘
                   └─────┬───────┘
                         │
          ┌──────────────┼──────────────┐
          │              │              │
     ┌────▼────┐    ┌────▼────┐    ┌────▼────┐
     │ Storage │    │  Index  │    │ Crypto  │
     │   CAS   │    │Database │    │Identity │
     │ Chunks  │    │ SQLite  │    │ Ed25519 │
     └─────────┘    └─────────┘    └─────────┘
```

### Modern Protocol Stack

```
┌─────────────────────────────────────┐
│      Application Layer              │ ← File sync, deduplication
├─────────────────────────────────────┤
│      Landropic Protocol             │ ← Custom sync protocol (protobuf)
├─────────────────────────────────────┤
│      QUIC Transport                 │ ← Multiplexed streams, 0-RTT
├─────────────────────────────────────┤
│      TLS 1.3 + mTLS                │ ← Mutual authentication
├─────────────────────────────────────┤
│      UDP + mDNS Discovery          │ ← Auto peer discovery
└─────────────────────────────────────┘
```

### Core Components

- **CLI (`landro-cli`)**: Command-line interface with progress tracking and configuration
- **Daemon (`landro-daemon`)**: Background orchestrator with sync engine and file watching
- **QUIC Transport (`landro-quic`)**: High-performance networking with mTLS and stream multiplexing
- **Cryptography (`landro-crypto`)**: Ed25519 device identity, X.509 certificates, SPAKE2 pairing
- **Content Store (`landro-cas`)**: Content-addressable storage with deduplication and compression
- **File Indexer (`landro-index`)**: File system monitoring with efficient change detection
- **Chunker (`landro-chunker`)**: FastCDC algorithm for optimal chunk boundaries
- **Protocol (`landro-proto`)**: Protocol Buffers for efficient message serialization
- **Sync Engine (`landro-sync`)**: High-level coordination with conflict resolution

## Configuration

Landropic stores configuration and data in these locations:

- **Linux**: `~/.config/landropic/` and `~/.local/share/landropic/`
- **macOS**: `~/Library/Application Support/landropic/`
- **Windows**: `%APPDATA%\landropic\`

Key files:

- `config.json`: Device configuration and settings
- `identity/`: Ed25519 device identity keys
- `objects/`: Content-addressable storage for file chunks
- `index.sqlite`: File metadata and sync state database

## Security Model

Landropic implements comprehensive defense-in-depth security:

1. **Device Identity**: Ed25519 keys with Blake3-based unique device IDs
2. **Secure Pairing**: SPAKE2-inspired PAKE with Argon2id key derivation
3. **Transport Security**: QUIC with TLS 1.3, mTLS, and certificate pinning
4. **Content Encryption**: Per-file ChaCha20-Poly1305 AEAD with unique keys
5. **Integrity Protection**: Blake3 hashing for chunks, manifests, and authentication
6. **Forward Secrecy**: Ephemeral X25519 key exchange for all sessions
7. **Network Isolation**: mDNS discovery limited to local network segments

## Development

### Prerequisites

- Rust 1.75 or later
- Protocol Buffers compiler (`protoc`)
- Platform-specific dependencies (see individual crate documentation)

### Building

```bash
# Development build
cargo build

# Optimized release build
cargo build --release

# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run --bin landropic-daemon
```

### Project Structure

```
landropic/
├── landro-cli/          # Command-line interface
├── landro-daemon/       # Background daemon
├── landro-quic/         # QUIC transport layer
├── landro-crypto/       # Cryptographic primitives
├── landro-cas/          # Content-addressable storage
├── landro-index/        # File indexing and watching
├── landro-chunker/      # Content-defined chunking
├── landro-proto/        # Protocol buffer definitions
├── landro-sync/         # Synchronization engine
├── docs/                # Documentation
└── tests/               # Integration tests
```

## Performance

Landropic is designed for high performance on local networks:

- **Chunked Transfer**: Only changed portions of files are synchronized
- **Parallel Processing**: Multiple concurrent chunk transfers
- **Efficient Storage**: Deduplication via content-addressable storage
- **Optimized Protocol**: QUIC's multiplexed streams eliminate head-of-line blocking

Expected performance on 1Gbps LAN:

- Large file transfers: ~800-900 Mbps throughput
- Small file changes: Sub-second propagation
- Discovery latency: <2 seconds for device detection

## Contributing

1. Check existing [issues](https://github.com/landropic/landropic/issues) or create a new one
2. Fork the repository and create a feature branch
3. Make your changes with tests and documentation
4. Ensure all tests pass: `cargo test`
5. Format code: `cargo fmt`
6. Check for issues: `cargo clippy`
7. Submit a pull request with a clear description

## License

Landropic is dual-licensed under either:

- **MIT License** ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)

at your option.

## Documentation

- 📚 [User Guide](docs/USER_GUIDE.md) - Getting started and everyday usage
- 🔧 [CLI Reference](docs/CLI_REFERENCE.md) - Complete command documentation
- 🏗️ [Developer Setup](docs/DEVELOPER_SETUP.md) - Development environment setup
- 📐 [API Reference](docs/API_REFERENCE.md) - Rust crate APIs and examples
- 🔒 [Security & Protocols](docs/PROTOCOLS_AND_SECURITY.md) - Cryptography and network protocols
- 🏛️ [Architecture](docs/architechture.md) - System design and decisions

## Support

- 📖 [Complete Documentation](docs/)
- 🐛 [Issue Tracker](https://github.com/landropic/landropic/issues)
- 💬 [Discussions](https://github.com/landropic/landropic/discussions)
- 🚀 [Quick Start Guide](docs/USER_GUIDE.md#getting-started)

---
