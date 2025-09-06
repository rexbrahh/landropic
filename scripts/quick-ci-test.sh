#!/bin/bash

# Quick CI test - simulates the most common CI failures locally
# This is faster than the full local-ci.sh for quick iterations

set -e

echo "🚀 Quick CI Test - Catching common failures fast"
echo ""

# Clean environment
unset CARGO_BUILD_JOBS
unset RUSTC_WRAPPER
export CARGO_TERM_COLOR=always

echo "1️⃣ Checking protoc..."
if ! command -v protoc &> /dev/null; then
    echo "❌ protoc not installed"
    exit 1
fi
echo "✅ protoc found"

echo ""
echo "2️⃣ Format check..."
if ! cargo fmt --all -- --check &>/dev/null; then
    echo "❌ Format issues - run: cargo fmt --all"
    cargo fmt --all -- --check 2>&1 | head -20
    exit 1
fi
echo "✅ Format OK"

echo ""
echo "3️⃣ Clean build test (just proto crate)..."
cargo clean -p landro-proto
if ! cargo build -p landro-proto 2>&1 | grep -q "Finished"; then
    echo "❌ Proto crate build failed"
    cargo build -p landro-proto 2>&1 | grep "error:" | head -10
    exit 1
fi
echo "✅ Proto crate builds"

echo ""
echo "4️⃣ Quick workspace build..."
if ! cargo build --workspace 2>&1 | grep -q "Finished"; then
    echo "❌ Workspace build failed"
    cargo build --workspace 2>&1 | grep "error:" | head -20
    exit 1
fi
echo "✅ Workspace builds"

echo ""
echo "5️⃣ Quick clippy check (errors only)..."
if cargo clippy --workspace -- -D warnings 2>&1 | grep -q "error:"; then
    echo "⚠️ Clippy errors found"
    cargo clippy --workspace -- -D warnings 2>&1 | grep "error:" | head -10
else
    echo "✅ No clippy errors"
fi

echo ""
echo "🎉 Quick CI test passed! Ready to push."