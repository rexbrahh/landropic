# Landropic Development Commands
# Install just: brew install just

# Show all available commands
default:
    @just --list

# CI/CD simulation commands

# Run all quality checks locally (matches CI pipeline)
ci-check:
    @echo "🚀 Running CI quality checks locally..."
    @echo "🔧 Step 1: Auto-fixing formatting..."
    cargo fmt --all
    @echo "🔍 Step 2: Checking formatting..."
    cargo fmt --all -- --check
    @echo "🔍 Step 3: Running Clippy analysis..."
    cargo clippy --workspace --all-targets -- -D warnings
    @echo "🧪 Step 4: Running fast tests..."
    cargo test --workspace --profile test-fast
    @echo "✅ All CI checks passed locally!"

# Auto-fix code quality issues
fix:
    @echo "🔧 Auto-fixing code quality issues..."
    cargo fmt --all
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged
    @echo "✅ Auto-fix completed!"

# Development builds (fast)
build-dev:
    cargo build --profile dev-fast

# Production build
build:
    cargo build --release

# Run all tests with fast profile
test-fast:
    cargo test --profile test-fast --workspace

# Run specific crate tests
test-crate crate:
    cargo test -p {{crate}} --profile test-fast

# Run tests for multiple crates in parallel
test-multi *crates:
    #!/usr/bin/env bash
    for crate in {{crates}}; do
        echo "Testing $crate..."
        cargo test -p "$crate" --profile test-fast &
    done
    wait

# Quick check (clippy + fmt + basic compilation)
check:
    cargo clippy --workspace --all-targets
    cargo fmt --all -- --check
    cargo check --workspace

# Fix common issues
fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty
    cargo fmt --all

# Watch mode for continuous testing
watch-test:
    cargo watch -x "test --profile test-fast --workspace"

# Watch specific crate
watch-crate crate:
    cargo watch -x "test -p {{crate}} --profile test-fast"

# Parallel development commands for multiple agents
dev-crypto:
    cargo watch -x "test -p landro-crypto --profile test-fast" -x "clippy -p landro-crypto"

dev-quic:
    cargo watch -x "test -p landro-quic --profile test-fast" -x "clippy -p landro-quic"

dev-storage:
    cargo watch -x "test -p landro-cas --profile test-fast" -x "test -p landro-chunker --profile test-fast"

dev-sync:
    cargo watch -x "test -p landro-index --profile test-fast" -x "test -p landro-daemon --profile test-fast"

# Clean and rebuild everything
reset:
    cargo clean
    cargo build --profile dev-fast

# Benchmark specific crates
bench crate:
    cargo bench -p {{crate}}

# Generate documentation
docs:
    cargo doc --workspace --no-deps --open

# Security audit
audit:
    cargo audit

# Coverage report
coverage:
    cargo tarpaulin --workspace --out html --output-dir coverage

# Integration test specific modules
integration module:
    cargo test --profile test-fast --test {{module}}

# Quick smoke test (basic functionality)
smoke:
    cargo test --profile test-fast --lib --bins

# Full CI pipeline locally
ci: check test-fast
    @echo "All checks passed! ✅"

# QA/Testing Agent Commands
# ========================

# Comprehensive testing workflow for QA agent
qa-full:
    @echo "🧪 Starting comprehensive QA validation..."
    cargo test --workspace --profile test-fast
    cargo test --workspace --release --quiet
    just integration-all
    just cross-platform-check

# Integration testing across all crate combinations
integration-all:
    #!/usr/bin/env bash
    echo "🔗 Running integration tests..."
    for crate in landro-proto landro-crypto landro-quic landro-chunker landro-cas landro-index landro-daemon landro-cli; do
        echo "Integration testing $crate..."
        cargo test -p "$crate" --profile test-fast --test '*' &
    done
    wait

# Cross-platform compatibility checks
cross-platform-check:
    @echo "🌐 Checking cross-platform compatibility..."
    cargo check --target x86_64-apple-darwin
    cargo check --target aarch64-apple-darwin

# Monitor all tests continuously
qa-monitor:
    cargo watch --clear --exec "test --workspace --profile test-fast"

# Performance regression detection
qa-performance:
    cargo bench --workspace
    @echo "📊 Check benchmark results for regressions"

# Security audit for QA agent
qa-security:
    cargo audit
    cargo clippy -- -D warnings

# Code Quality Agent Commands  
# ===========================

# Comprehensive quality improvements
quality-sweep:
    @echo "✨ Starting code quality sweep..."
    just fix-clippy
    just fix-fmt
    just quality-check
    just dead-code-removal

# Fix all clippy warnings
fix-clippy:
    cargo clippy --workspace --all-targets --fix --allow-dirty

# Apply consistent formatting
fix-fmt:
    cargo fmt --all

# Detect and report quality issues
quality-check:
    @echo "🔍 Quality analysis..."
    cargo clippy --workspace -- -D warnings
    cargo fmt --all -- --check
    @echo "📦 Checking for unused dependencies..."
    @# Could add cargo-udeps here: cargo +nightly udeps

# Find and remove dead code
dead-code-removal:
    @echo "🧹 Checking for dead code..."
    cargo check --workspace
    @echo "Run 'cargo +nightly rustc -- -Z unused-features' manually for unused features"

# Performance optimization focus
quality-performance:
    @echo "⚡ Performance optimization check..."
    cargo build --release
    cargo bench --workspace

# Dependency analysis and updates
quality-deps:
    @echo "📦 Dependency analysis..."
    cargo tree --duplicates
    @echo "Consider running 'cargo update' to update dependencies"

# Code complexity analysis
quality-complexity:
    @echo "📊 Code complexity analysis..."
    @echo "Lines of code by crate:"
    @find . -name "*.rs" -path "*/src/*" | xargs wc -l | sort -n

# Agent coordination commands
# ==========================

# Show current agent assignments and status
agent-status:
    @echo "🤖 Multi-Agent Status Dashboard"
    @echo "================================="
    @cat .agent-status | grep -E '\[AGENT|status:|working_on:|last_update:|specialization:' | sed 's/\[AGENT/\n[AGENT/g'

# CI/CD Agent Commands
# ====================

# CI/CD Agent primary workflow - orchestrates the entire pipeline
cicd-orchestrate:
    @echo "🚀 CI/CD Agent: Orchestrating full pipeline..."
    just cicd-pre-check
    just cicd-validate-agents
    just cicd-run-tests
    just cicd-quality-gates
    just cicd-integration-check
    @echo "✅ CI/CD orchestration complete"

# Pre-flight checks before pipeline execution
cicd-pre-check:
    @echo "🔍 CI/CD Pre-flight checks..."
    git status --porcelain
    cargo check --workspace --quiet
    @echo "✅ Pre-flight complete"

# Validate that all agents are in stable states
cicd-validate-agents:
    @echo "🤖 Validating agent states..."
    @echo "Checking agent status board..."
    @grep -E "status: (blocked|error)" .agent-status && echo "❌ Found blocked/error agents" && exit 1 || echo "✅ All agents stable"

# Comprehensive test execution coordinated with QA agent
cicd-run-tests:
    @echo "🧪 CI/CD + QA Agent: Comprehensive testing..."
    just test-fast
    just integration-all
    cargo test --workspace --release --quiet

# Quality gates coordinated with Code Quality agent
cicd-quality-gates:
    @echo "✨ CI/CD + Quality Agent: Quality gates..."
    just quality-check
    just qa-security
    cargo clippy --workspace -- -D warnings

# Integration readiness check
cicd-integration-check:
    @echo "🔗 Integration readiness check..."
    cargo build --workspace --release
    cargo doc --workspace --no-deps --quiet
    @echo "✅ Integration ready"

# Release preparation workflow
cicd-release-prep:
    @echo "📦 CI/CD Agent: Release preparation..."
    just cicd-orchestrate
    just qa-performance
    just quality-deps
    cargo audit
    @echo "🚀 Release preparation complete"

# Deployment readiness validation
cicd-deploy-ready:
    @echo "🚀 Deployment readiness validation..."
    just cicd-release-prep
    @echo "Verifying cross-platform compatibility..."
    just cross-platform-check
    @echo "✅ Ready for deployment"

# CI/CD Integration Commands
# ==========================

# Local CI pipeline (matches GitHub Actions)
local-ci:
    @echo "🚀 Running local CI pipeline..."
    just check
    just test-fast
    cargo test --workspace --release --quiet
    just qa-security
    @echo "✅ Local CI pipeline completed"

# QA Agent CI/CD automation
qa-ci-automation:
    @echo "🧪 QA Agent CI/CD Automation..."
    just integration-all
    just qa-security
    just qa-performance
    cargo tarpaulin --workspace --out html --output-dir coverage || echo "Coverage completed with warnings"
    @echo "📊 QA automation complete - check coverage/ directory"

# Code Quality CI/CD automation  
quality-ci-automation:
    @echo "✨ Quality Agent CI/CD Automation..."
    cargo clippy --workspace --all-targets --fix --allow-dirty
    cargo fmt --all
    just quality-deps
    just dead-code-removal
    @echo "🎯 Quality automation complete"

# Pre-commit validation (for all agents)
pre-commit:
    @echo "🔍 Pre-commit validation..."
    git status --porcelain
    just check
    just test-fast
    just qa-security
    @echo "✅ Ready to commit"

# Pre-push validation (comprehensive)
pre-push:
    @echo "🚀 Pre-push validation..."
    just local-ci
    just qa-ci-automation
    @echo "✅ Ready to push"

# Release readiness check
release-ready:
    @echo "📦 Release readiness check..."
    cargo build --release --workspace
    cargo test --workspace --release
    cargo doc --workspace --no-deps
    just qa-full
    just quality-sweep
    @echo "🚀 Release ready!"