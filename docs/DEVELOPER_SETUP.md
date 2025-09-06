# Landropic Developer Setup Guide

## Overview

This guide walks through setting up a development environment for contributing to Landropic, including all prerequisites, build tools, and recommended development practices.

## Prerequisites

### System Requirements

#### Minimum Requirements
- **Operating System**: Linux, macOS, or Windows (WSL2 recommended for Windows)
- **Memory**: 4GB RAM (8GB recommended for compilation)
- **Storage**: 2GB free space for source code and build artifacts
- **Network**: Internet connection for dependency downloads

#### Supported Platforms
- **Linux**: Ubuntu 20.04+, CentOS 8+, Arch Linux
- **macOS**: macOS 11.0+ (Big Sur and later)
- **Windows**: Windows 10/11 with WSL2, or native Windows with MSVC

### Required Tools

#### 1. Rust Toolchain

Landropic requires Rust 1.75.0 or later with specific components:

```bash
# Install rustup (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install stable Rust (1.75+)
rustup install stable
rustup default stable

# Add required components
rustup component add clippy rustfmt
```

**Verify installation:**
```bash
rustc --version  # Should show 1.75.0+
cargo --version
```

#### 2. Protocol Buffers Compiler

Required for compiling `.proto` files in `landro-proto`:

**Ubuntu/Debian:**
```bash
sudo apt update
sudo apt install protobuf-compiler libprotobuf-dev
```

**macOS:**
```bash
# Using Homebrew
brew install protobuf

# Or using MacPorts
sudo port install protobuf3-cpp
```

**Windows (WSL2):**
```bash
sudo apt update
sudo apt install protobuf-compiler libprotobuf-dev
```

**Verify installation:**
```bash
protoc --version  # Should show libprotoc 3.12.0 or later
```

#### 3. Platform-Specific Dependencies

**Linux (Ubuntu/Debian):**
```bash
# Core build tools
sudo apt install build-essential pkg-config

# Networking and crypto libraries
sudo apt install libssl-dev

# Optional: Development tools
sudo apt install git curl gdb valgrind
```

**macOS:**
```bash
# Install Xcode Command Line Tools
xcode-select --install

# Optional: Development tools via Homebrew
brew install git curl lldb
```

**Windows (Native):**
- Install Visual Studio 2019+ with C++ build tools
- Install Git for Windows
- Install Windows SDK

### Optional but Recommended

#### Development Tools

```bash
# Fast directory searching
cargo install fd-find

# Fast file content searching  
cargo install ripgrep

# Enhanced `ls` replacement
cargo install exa

# JSON processing
cargo install jq

# Process monitoring
cargo install bottom
```

#### IDE/Editor Setup

**VS Code Extensions:**
- `rust-analyzer` - Rust language server
- `CodeLLDB` - Debugging support
- `crates` - Crate dependency management
- `Error Lens` - Inline error display

**Vim/Neovim:**
- Configure LSP with `rust-analyzer`
- Install `nvim-cmp` for code completion

---

## Project Setup

### 1. Clone Repository

```bash
git clone https://github.com/landropic/landropic.git
cd landropic

# Set up development branch
git checkout -b feature/your-feature-name
```

### 2. Initial Build

```bash
# Build all crates in debug mode
cargo build

# Build specific crate
cargo build -p landro-quic

# Build with optimizations (release mode)
cargo build --release
```

**Expected build time:**
- Debug build: 5-15 minutes (first build)
- Incremental builds: 30 seconds - 2 minutes
- Release build: 10-30 minutes

### 3. Run Tests

```bash
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p landro-crypto

# Run with output
cargo test -- --nocapture

# Run integration tests
cargo test --test integration
```

### 4. Verify Setup

```bash
# Check formatting
cargo fmt --check

# Run linting
cargo clippy -- -D warnings

# Build documentation
cargo doc --no-deps --open

# Run example
cargo run --bin landropic-daemon --example simple_server
```

---

## Development Environment

### Cargo Configuration

Create `~/.cargo/config.toml` for optimal development experience:

```toml
[build]
# Use all CPU cores for compilation
jobs = 8

# Link faster on Linux
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]

# Link faster on macOS  
[target.x86_64-apple-darwin]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]

[alias]
# Useful aliases
b = "build"
c = "check"
t = "test"
r = "run"
f = "fmt"
l = "clippy"
```

### Environment Variables

Set these in your shell profile (`~/.bashrc`, `~/.zshrc`, etc.):

```bash
# Rust development
export RUST_BACKTRACE=1              # Full backtraces on panic
export RUST_LOG=debug                # Default log level
export CARGO_INCREMENTAL=1           # Enable incremental compilation

# Landropic development
export LANDROPIC_DEV_MODE=1          # Enable development features
export LANDROPIC_CONFIG_DIR="./dev-config"  # Use local config
export LANDROPIC_DATA_DIR="./dev-data"      # Use local data

# Optional: Faster builds (if you have enough RAM)
export CARGO_BUILD_JOBS=8             # Parallel build jobs
```

### Directory Structure

```
landropic/
├── Cargo.toml              # Workspace definition
├── Cargo.lock              # Dependency lock file  
├── README.md               # Project overview
├── LICENSE-MIT             # MIT license
├── LICENSE-APACHE          # Apache 2.0 license
│
├── landro-cli/             # Command-line interface
├── landro-daemon/          # Background daemon
├── landro-quic/            # QUIC transport layer
├── landro-crypto/          # Cryptographic primitives
├── landro-cas/             # Content-addressable storage
├── landro-index/           # File indexing and watching
├── landro-chunker/         # Content-defined chunking
├── landro-proto/           # Protocol buffer definitions
├── landro-sync/            # Synchronization engine
│
├── docs/                   # Documentation
│   ├── API_REFERENCE.md
│   ├── CLI_REFERENCE.md
│   └── ...
│
├── tests/                  # Integration tests
├── examples/               # Usage examples
├── scripts/                # Development scripts
└── .github/                # GitHub Actions CI/CD
```

---

## Development Workflow

### 1. Feature Development

```bash
# Create feature branch
git checkout -b feature/your-feature

# Write code and tests
# ... development work ...

# Check code quality
cargo fmt
cargo clippy -- -D warnings
cargo test

# Commit changes
git add .
git commit -m "feat: add your feature description"

# Push and create PR
git push origin feature/your-feature
```

### 2. Testing

#### Unit Tests
```bash
# Run all unit tests
cargo test

# Run specific test
cargo test test_device_identity

# Run tests with output
cargo test -- --nocapture --test-threads=1
```

#### Integration Tests
```bash
# Run integration tests
cargo test --test '*'

# Run specific integration test
cargo test --test quic_integration
```

#### Manual Testing
```bash
# Start daemon in development mode
RUST_LOG=debug cargo run --bin landro-daemon

# Use CLI in another terminal
cargo run --bin landropic -- init --name "Dev Device"
cargo run --bin landropic -- sync ./test-folder --watch
```

### 3. Debugging

#### Using `rust-gdb` / `lldb`
```bash
# Build with debug symbols
cargo build

# Debug specific binary
rust-gdb target/debug/landro-daemon
# or
rust-lldb target/debug/landro-daemon
```

#### Using IDE Debuggers
Configure your IDE to run:
```json
{
    "type": "lldb",
    "request": "launch",
    "name": "Debug daemon",
    "cargo": {
        "args": ["build", "--bin=landro-daemon"],
        "filter": {
            "name": "landro-daemon",
            "kind": "bin"
        }
    },
    "args": [],
    "cwd": "${workspaceFolder}"
}
```

#### Logging and Tracing
```rust
use tracing::{debug, info, warn, error};

fn example_function() {
    debug!("Detailed debug information");
    info!("General information");
    warn!("Warning message");
    error!("Error occurred");
}
```

**Run with specific log levels:**
```bash
RUST_LOG=landro_quic=debug,landro_daemon=info cargo run
```

### 4. Performance Profiling

#### CPU Profiling with `perf`
```bash
# Install perf tools (Linux)
sudo apt install linux-perf

# Profile application
sudo perf record --call-graph=dwarf cargo run --release --bin landro-daemon
sudo perf report
```

#### Memory Profiling with `valgrind`
```bash
# Install valgrind (Linux)
sudo apt install valgrind

# Check for memory leaks
cargo build
valgrind --leak-check=full --track-origins=yes ./target/debug/landro-daemon
```

---

## Common Issues and Solutions

### 1. Compilation Errors

**Problem**: "error: linker `cc` not found"
**Solution**:
```bash
# Ubuntu/Debian
sudo apt install build-essential

# macOS
xcode-select --install
```

**Problem**: "error: Microsoft Visual C++ 14.0 is required" (Windows)
**Solution**: Install Visual Studio Build Tools 2019+

### 2. Protocol Buffer Issues

**Problem**: "protoc: command not found"
**Solution**:
```bash
# Install protobuf compiler (see prerequisites section)
```

**Problem**: "error: failed to run custom build command for landro-proto"
**Solution**:
```bash
# Verify protoc version
protoc --version

# Clean and rebuild
cargo clean
cargo build -p landro-proto
```

### 3. Test Failures

**Problem**: Tests fail with network errors
**Solution**:
```bash
# Run tests with fewer threads to avoid port conflicts
cargo test -- --test-threads=1

# Run specific failing test with output
cargo test test_name -- --nocapture
```

### 4. Performance Issues

**Problem**: Slow compilation times
**Solutions**:
```bash
# Use faster linker (Linux)
sudo apt install lld
export RUSTFLAGS="-C link-arg=-fuse-ld=lld"

# Enable incremental compilation
export CARGO_INCREMENTAL=1

# Use more build jobs
cargo build -j 8
```

---

## Code Style and Standards

### Formatting

Always use `rustfmt` before committing:
```bash
# Format all code
cargo fmt

# Check formatting without modifying
cargo fmt --check
```

### Linting

Use `clippy` for code quality:
```bash
# Run clippy
cargo clippy

# Treat warnings as errors
cargo clippy -- -D warnings

# Apply automatic fixes
cargo clippy --fix
```

### Documentation

Write comprehensive documentation:
```rust
/// Brief description of the function.
/// 
/// More detailed explanation of what this function does,
/// including any important implementation notes.
/// 
/// # Arguments
/// 
/// * `param1` - Description of first parameter
/// * `param2` - Description of second parameter
/// 
/// # Returns
/// 
/// Description of return value
/// 
/// # Errors
/// 
/// Description of when this function might return an error
/// 
/// # Examples
/// 
/// ```
/// use landro_crypto::DeviceIdentity;
/// 
/// let identity = DeviceIdentity::generate("test")?;
/// assert!(!identity.device_id().to_string().is_empty());
/// ```
pub fn example_function(param1: &str, param2: u32) -> Result<String> {
    // Implementation
}
```

### Testing Standards

Write comprehensive tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_basic_functionality() {
        // Test basic case
        let result = function_under_test("input");
        assert_eq!(result, expected_output);
    }
    
    #[test]
    fn test_error_conditions() {
        // Test error cases
        let result = function_under_test("invalid");
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_async_functionality() {
        // Test async functions
        let result = async_function().await;
        assert!(result.is_ok());
    }
}
```

---

## Getting Help

### Documentation
- **API Docs**: Run `cargo doc --open`  
- **Book**: [Rust Book](https://doc.rust-lang.org/book/)
- **Async Book**: [Async Programming in Rust](https://rust-lang.github.io/async-book/)

### Community
- **Issues**: [GitHub Issues](https://github.com/landropic/landropic/issues)
- **Discussions**: [GitHub Discussions](https://github.com/landropic/landropic/discussions)
- **Rust Community**: [Users Forum](https://users.rust-lang.org/)

### Development Resources
- **Rust Reference**: https://doc.rust-lang.org/reference/
- **Cargo Book**: https://doc.rust-lang.org/cargo/
- **Tokio Guide**: https://tokio.rs/tokio/tutorial

---

## Next Steps

After completing setup:

1. **Read the codebase**: Start with `README.md` and architecture docs
2. **Find good first issues**: Look for issues labeled "good-first-issue"
3. **Join discussions**: Participate in GitHub discussions
4. **Run integration tests**: Verify everything works end-to-end
5. **Make your first contribution**: Fix a bug or add a small feature

Welcome to the Landropic development community!