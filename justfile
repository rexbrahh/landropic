# Landropic build tasks (Nix-powered)
# Run 'just' to see all available commands

default:
    @just --list

# Enter development shell
dev:
    nix develop

# Build all packages
build:
    nix build

# Build specific package
build-pkg PKG:
    nix build .#{{PKG}}

# Run daemon
daemon:
    nix run .#daemon

# Run CLI
cli *ARGS:
    nix run .#cli -- {{ARGS}}

# Run all tests
test:
    cargo nextest run

# Run benchmarks
bench:
    nix develop .#bench -c cargo criterion

# Check code formatting
fmt-check:
    cargo fmt --check
    nixpkgs-fmt --check .

# Format code
fmt:
    cargo fmt
    nixpkgs-fmt .

# Run clippy
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Full CI check
ci: fmt-check clippy test
    nix flake check

# Build for all platforms
build-all:
    nix build .#landropic
    nix build .#landropic-static
    nix build .#landropic-aarch64-linux
    nix build .#docker

# Build Docker image
docker:
    nix build .#docker
    docker load < result

# Create release
release VERSION:
    #!/usr/bin/env bash
    set -e
    echo "Creating release {{VERSION}}"
    
    # Update version in Cargo.toml
    cargo set-version {{VERSION}}
    
    # Build all targets
    nix build .#landropic
    nix build .#landropic-static
    nix build .#docker
    
    # Create release directory
    mkdir -p releases/{{VERSION}}
    
    # Copy artifacts
    cp -r result/* releases/{{VERSION}}/
    cp result releases/{{VERSION}}/docker.tar.gz
    
    echo "Release {{VERSION}} created in releases/{{VERSION}}"

# Deploy to NixOS machine
deploy HOST:
    nixos-rebuild switch --flake .#{{HOST}} --target-host {{HOST}}

# Setup development environment
setup:
    nix run .#dev-setup

# Clean build artifacts
clean:
    cargo clean
    rm -rf result* releases/

# Update dependencies
update:
    nix flake update
    cargo update

# Security audit
audit:
    cargo audit
    cargo deny check

# Code coverage
coverage:
    nix develop -c cargo tarpaulin --out Html

# Profile with flamegraph
profile CMD:
    nix develop .#bench -c cargo flamegraph --bin landro-daemon -- {{CMD}}

# Watch for changes and rebuild
watch:
    bacon

# Generate documentation
docs:
    cargo doc --no-deps --open
    mdbook build docs/

# Run with Valgrind (memory debugging)
valgrind:
    nix develop -c valgrind --leak-check=full --show-leak-kinds=all target/debug/landro-daemon

# Cross-compile for Linux
cross-linux:
    nix build .#landropic-aarch64-linux
    nix build .#landropic-static

# Cross-compile for Windows (experimental)
cross-windows:
    nix build .#landropic-windows

# Push to Cachix
cache-push:
    nix build --json | jq -r '.[].outputs.out' | cachix push landropic

# Install locally
install:
    nix profile install .#landropic

# Uninstall
uninstall:
    nix profile remove landropic