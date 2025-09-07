#!/bin/bash

# Local CI Simulation Script
# This script runs the same checks as GitHub Actions locally
# Usage: ./scripts/local-ci.sh [all|format|clippy|build|test]

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_step() {
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}▶ $1${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

# Clean environment variables that might interfere
clean_env() {
    unset CARGO_BUILD_JOBS
    unset RUSTC_WRAPPER
    export CARGO_TERM_COLOR=always
    export RUST_BACKTRACE=1
}

# Function to check prerequisites
check_prerequisites() {
    print_step "Checking prerequisites"
    
    # Check for rustc
    if ! command -v rustc &> /dev/null; then
        print_error "Rust is not installed"
        exit 1
    fi
    print_success "Rust $(rustc --version)"
    
    # Check for protoc
    if ! command -v protoc &> /dev/null; then
        print_error "protoc is not installed"
        echo "Install with: brew install protobuf (macOS) or apt-get install protobuf-compiler (Linux)"
        exit 1
    fi
    print_success "protoc $(protoc --version)"
    
    # Check for rustfmt
    if ! command -v rustfmt &> /dev/null; then
        print_warning "rustfmt is not installed"
        echo "Install with: rustup component add rustfmt"
    fi
    
    # Check for clippy
    if ! command -v cargo-clippy &> /dev/null; then
        print_warning "clippy is not installed"
        echo "Install with: rustup component add clippy"
    fi
}

# Function to run format check
run_format_check() {
    print_step "Format Check (cargo fmt)"
    
    if cargo fmt --all -- --check; then
        print_success "Code formatting is correct"
    else
        print_error "Code formatting issues found!"
        echo ""
        echo "To fix: cargo fmt --all"
        echo ""
        echo "Showing diff:"
        cargo fmt --all -- --check --verbose 2>&1 || true
        return 1
    fi
}

# Function to run clippy
run_clippy() {
    print_step "Clippy Analysis"
    
    if cargo clippy --workspace --all-targets -- \
        -D warnings \
        -W clippy::cargo \
        -W clippy::nursery \
        -W clippy::pedantic \
        -A clippy::module_name_repetitions \
        -A clippy::missing_errors_doc; then
        print_success "Clippy analysis passed"
    else
        print_error "Clippy found issues"
        return 1
    fi
}

# Function to run clean build
run_clean_build() {
    print_step "Clean Build Test"
    
    # Clean build directory
    print_warning "Cleaning target directory..."
    cargo clean
    
    # Build all workspace members
    if cargo build --workspace; then
        print_success "Build successful"
    else
        print_error "Build failed"
        return 1
    fi
}

# Function to run tests
run_tests() {
    print_step "Running Tests"
    
    # Run tests with different profiles
    echo "Running fast tests..."
    if cargo test --workspace --lib --bins; then
        print_success "Fast tests passed"
    else
        print_error "Fast tests failed"
        return 1
    fi
    
    echo "Running doc tests..."
    if cargo test --workspace --doc; then
        print_success "Doc tests passed"
    else
        print_warning "Doc tests had issues"
    fi
}

# Function to run all checks
run_all() {
    local failed=0
    
    run_format_check || failed=1
    run_clippy || failed=1
    run_clean_build || failed=1
    run_tests || failed=1
    
    if [ $failed -eq 0 ]; then
        print_success "All CI checks passed! 🎉"
    else
        print_error "Some CI checks failed"
        exit 1
    fi
}

# Main script
main() {
    echo -e "${GREEN}🚀 Local CI Simulation${NC}"
    echo "This simulates GitHub Actions CI pipeline locally"
    echo ""
    
    clean_env
    check_prerequisites
    
    case "${1:-all}" in
        format)
            run_format_check
            ;;
        clippy)
            run_clippy
            ;;
        build)
            run_clean_build
            ;;
        test)
            run_tests
            ;;
        all)
            run_all
            ;;
        *)
            echo "Usage: $0 [all|format|clippy|build|test]"
            echo ""
            echo "Commands:"
            echo "  all     - Run all CI checks (default)"
            echo "  format  - Check code formatting"
            echo "  clippy  - Run clippy lints"
            echo "  build   - Clean build test"
            echo "  test    - Run test suite"
            exit 1
            ;;
    esac
}

# Run main function
main "$@"