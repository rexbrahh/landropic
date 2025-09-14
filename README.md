# Landropic

**Cross-platform encrypted LAN file sync - AirDrop for everyone**

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org) 
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://github.com/landropic/landropic#license)
[![Nix](https://img.shields.io/badge/Built%20with-Nix-blue.svg)](https://nixos.org)
[![CI](https://github.com/rexbrahh/landropic/actions/workflows/ci.yml/badge.svg)](https://github.com/rexbrahh/landropic/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/rexbrahh/landropic?include_prereleases)](https://github.com/rexbrahh/landropic/releases)

Landropic is a secure, fast, and reliable file synchronization tool designed for local networks. It provides end-to-end encryption, automatic device discovery, and efficient chunked file transfer using modern protocols.

## Features

- **🔒 End-to-End Encryption**: Files are encrypted using ChaCha20-Poly1305 with Ed25519 device authentication
- **🚀 High Performance**: QUIC-based transport with content-defined chunking for optimal throughput
- **🔍 Auto Discovery**: Automatic device discovery using mDNS/Zeroconf
- **📁 Real-time Sync**: Continuous file watching and synchronization
- **⚡ Resume Support**: Interrupted transfers automatically resume from where they left off
- **🖥️ Cross Platform**: Works on Linux, macOS, and Windows
- **📊 Progress Tracking**: Real-time sync progress with detailed status reporting
- **❄️ Reproducible Builds**: Fully reproducible builds with Nix

## Quick Start

### Installation Options

#### Option 1: Pre-built Binaries (Recommended)
Download the latest release for your platform from the [releases page](https://github.com/rexbrahh/landropic/releases).

```bash
# macOS/Linux
curl -L https://github.com/rexbrahh/landropic/releases/latest/download/landropic-macos.tar.gz | tar xz
sudo mv landro-* /usr/local/bin/
```

#### Option 2: Install with Nix (Best for Developers)
```bash
# Install directly
nix profile install github:rexbrahh/landropic

# Or run without installing
nix run github:rexbrahh/landropic#daemon
nix run github:rexbrahh/landropic#cli -- --help
```

#### Option 3: Build from Source
```bash
# With Nix (recommended - handles all dependencies)
git clone https://github.com/rexbrahh/landropic.git
cd landropic
nix build
./result/bin/landro-daemon

# With Cargo (requires Rust toolchain)
cargo build --release
./target/release/landro-daemon
```

#### Option 4: Docker
```bash
# Using pre-built image
docker run -d \
  -p 9990:9990/tcp \
  -p 9990:9990/udp \
  -v ~/sync:/sync \
  rexbrahh/landropic:latest

# Or build with Nix
nix build .#docker
docker load < result
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

## Development

### Prerequisites

#### Using Nix (Recommended - Zero Setup)
```bash
# Install Nix
curl -L https://nixos.org/nix/install | sh

# Enter development environment with ALL tools
nix develop

# That's it! You now have:
# - Rust toolchain with all targets
# - All build dependencies
# - Development tools (cargo-watch, bacon, etc.)
# - Testing tools (nextest, tarpaulin)
# - Debugging tools (gdb, lldb, valgrind)
```

#### Manual Setup (Without Nix)
- Rust 1.75 or later
- Protocol Buffers compiler (`protoc`)
- OpenSSL development headers
- SQLite development headers

### Building

```bash
# Using Nix (reproducible builds)
nix build                    # Native build
nix build .#landropic-static # Static Linux binary
nix build .#docker          # Docker image

# Using Just (task runner)
just build      # Build everything
just test       # Run tests
just bench      # Run benchmarks
just release v0.1.0  # Create release

# Using Cargo directly
cargo build --release
cargo test
```

### Development Workflow

```bash
# Auto-reload development environment
direnv allow  # One-time setup

# Available development commands
just          # Show all tasks
just watch    # Watch mode with bacon
just fmt      # Format code
just clippy   # Run linter
just coverage # Generate code coverage
just profile  # Performance profiling
```

### Cross-Platform Builds

```bash
# Build for all platforms from any platform with Nix
nix build .#landropic                # Native
nix build .#landropic-static         # Linux static
nix build .#landropic-aarch64-linux  # ARM64 Linux
nix build .#landropic-windows        # Windows (experimental)
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

### Core Components

- **CLI (`landro-cli`)**: Command-line interface with progress tracking
- **Daemon (`landro-daemon`)**: Background orchestrator with sync engine
- **QUIC Transport (`landro-quic`)**: High-performance networking with mTLS
- **Content Store (`landro-cas`)**: Content-addressable storage with deduplication
- **File Indexer (`landro-index`)**: File system monitoring with change detection
- **Chunker (`landro-chunker`)**: FastCDC algorithm for optimal chunk boundaries

## Deployment

### NixOS Module

Add to your NixOS configuration:
```nix
{
  services.landropic = {
    enable = true;
    syncDirs = [ "/home/user/Documents" ];
    openFirewall = true;
  };
}
```

### Home Manager

```nix
{
  services.landropic = {
    enable = true;
    syncDirs = [ "Documents" "Pictures" ];
  };
}
```

### Systemd (Linux)

```bash
# Install the systemd service
sudo cp result/lib/systemd/user/landropic.service /etc/systemd/user/
systemctl --user enable --now landropic
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

## Security Model

Landropic implements defense-in-depth security:

1. **Device Identity**: Ed25519 keys with unique device IDs
2. **Secure Pairing**: PAKE-based pairing with Argon2id key derivation
3. **Transport Security**: QUIC with TLS 1.3 and mutual authentication
4. **Content Encryption**: Per-file ChaCha20-Poly1305 AEAD
5. **Integrity Protection**: Blake3 hashing throughout
6. **Network Isolation**: mDNS discovery limited to local network

## Project Status

**Current Version**: v0.0.1-alpha (simplified scope for v1.0)

This is an early alpha release with a reduced feature set focused on core functionality:
- ✅ Basic QUIC networking
- ✅ Content-addressed storage
- ✅ Simple sync engine
- ✅ macOS support
- 🚧 Windows/Linux support (in progress)
- 🚧 GUI application (planned)

## Documentation

- 📚 [User Guide](docs/USER_GUIDE.md) - Getting started and everyday usage
- 🔧 [CLI Reference](docs/CLI_REFERENCE.md) - Complete command documentation
- ❄️ [Nix Guide](docs/NIX-GUIDE.md) - Development with Nix
- 🏗️ [Developer Setup](docs/DEVELOPER_SETUP.md) - Development environment
- 🔒 [Security & Protocols](docs/PROTOCOLS_AND_SECURITY.md) - Cryptography details
- 🏛️ [Architecture](docs/architechture.md) - System design decisions

## Contributing

1. Check existing [issues](https://github.com/rexbrahh/landropic/issues) or create a new one
2. Fork the repository and create a feature branch
3. Make your changes with tests and documentation
4. Run checks: `just ci` (or `nix flake check`)
5. Submit a pull request with a clear description

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)

## Support

- 📖 [Complete Documentation](docs/)
- 🐛 [Issue Tracker](https://github.com/rexbrahh/landropic/issues)
- 💬 [Discussions](https://github.com/rexbrahh/landropic/discussions)
- 🚀 [Quick Start Guide](docs/USER_GUIDE.md#getting-started)

## Acknowledgments

Built with excellent open source projects:
- [Quinn](https://github.com/quinn-rs/quinn) - QUIC implementation
- [Tokio](https://tokio.rs) - Async runtime
- [Blake3](https://github.com/BLAKE3-team/BLAKE3) - Cryptographic hashing
- [Nix](https://nixos.org) - Reproducible builds

---

*Landropic: Making file sync simple, secure, and fast.*
