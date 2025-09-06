#!/bin/bash
set -e

# Release script using Nix for reproducible builds

VERSION=${1:-$(git describe --tags --always)}
PLATFORMS=${2:-"macos linux"}

echo "🚀 Building Landropic $VERSION"

# Ensure we have Nix
if ! command -v nix &> /dev/null; then
    echo "Installing Nix..."
    curl -L https://nixos.org/nix/install | sh
    source ~/.nix-profile/etc/profile.d/nix.sh
fi

# Build with Nix
echo "Building native binaries..."
nix build .#landropic

# Build Docker image (Linux only)
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "Building Docker image..."
    nix build .#dockerImage
    docker load < result
    docker tag landropic:latest landropic:$VERSION
fi

# Create release archives
mkdir -p releases
for platform in $PLATFORMS; do
    echo "Packaging for $platform..."
    if [ "$platform" = "macos" ]; then
        tar -czf releases/landropic-${VERSION}-macos.tar.gz -C result/bin .
    elif [ "$platform" = "linux" ]; then
        # Cross-compile for Linux if on macOS
        nix build .#packages.x86_64-linux.landropic
        tar -czf releases/landropic-${VERSION}-linux.tar.gz -C result/bin .
    fi
done

echo "✅ Release artifacts in ./releases/"
ls -la releases/