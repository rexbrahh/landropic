#!/bin/bash
# Integration test runner for Landropic
# This script builds the necessary components and runs integration tests

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_TIMEOUT=300  # 5 minutes
BUILD_MODE="${BUILD_MODE:-debug}"  # debug or release
RUST_LOG="${RUST_LOG:-info}"

echo -e "${BLUE}🚀 Starting Landropic Integration Tests${NC}"
echo "Workspace: $WORKSPACE_ROOT"
echo "Build mode: $BUILD_MODE"
echo "Rust log level: $RUST_LOG"
echo

# Function to print status messages
print_status() {
    echo -e "${BLUE}==>${NC} $1"
}

print_success() {
    echo -e "${GREEN}✅${NC} $1"
}

print_error() {
    echo -e "${RED}❌${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠️${NC} $1"
}

# Change to workspace root
cd "$WORKSPACE_ROOT"

# Step 1: Build all required binaries
print_status "Building required binaries..."

if [ "$BUILD_MODE" = "release" ]; then
    BUILD_FLAGS="--release"
    TARGET_DIR="target/release"
else
    BUILD_FLAGS=""
    TARGET_DIR="target/debug"
fi

# Build daemon
print_status "Building landro-daemon..."
if cargo build $BUILD_FLAGS -p landro-daemon; then
    print_success "landro-daemon built successfully"
else
    print_error "Failed to build landro-daemon"
    exit 1
fi

# Build CLI
print_status "Building landro-cli..."
if cargo build $BUILD_FLAGS -p landro-cli; then
    print_success "landro-cli built successfully"
else
    print_error "Failed to build landro-cli"
    exit 1
fi

# Verify binaries exist
DAEMON_BINARY="$TARGET_DIR/landro-daemon"
CLI_BINARY="$TARGET_DIR/landro-cli"

if [ ! -f "$DAEMON_BINARY" ]; then
    print_error "Daemon binary not found at $DAEMON_BINARY"
    exit 1
fi

if [ ! -f "$CLI_BINARY" ]; then
    print_error "CLI binary not found at $CLI_BINARY"
    exit 1
fi

print_success "All binaries built and verified"
echo

# Step 2: Set up test environment
print_status "Setting up test environment..."

# Clean up any existing test processes
print_status "Cleaning up existing test processes..."
pkill -f "landro-daemon.*test" || true
sleep 1

# Clean up test directories
TEST_DIRS=(
    "/tmp/landropic-test*"
    "/tmp/landro-*"
)

for pattern in "${TEST_DIRS[@]}"; do
    rm -rf $pattern 2>/dev/null || true
done

print_success "Test environment cleaned"

# Step 3: Run integration tests
print_status "Running integration tests..."

export RUST_LOG="$RUST_LOG"
export LANDROPIC_TEST_MODE=1

# List of test modules to run
INTEGRATION_TESTS=(
    "storage_test"
    "sync_test"  
    "network_test"
    "cli_daemon_test"
)

# Track test results
PASSED_TESTS=()
FAILED_TESTS=()

# Function to run a specific test module
run_test_module() {
    local test_module="$1"
    local test_name="tests::integration::${test_module}"
    
    print_status "Running $test_module..."
    
    # Create log file for this test
    local log_file="/tmp/landropic_test_${test_module}.log"
    
    # Run the test with timeout
    if timeout $TEST_TIMEOUT cargo test --package landropic --test lib integration::${test_module} -- --nocapture > "$log_file" 2>&1; then
        print_success "$test_module PASSED"
        PASSED_TESTS+=("$test_module")
    else
        print_error "$test_module FAILED"
        FAILED_TESTS+=("$test_module")
        
        # Show last 20 lines of error log
        echo -e "${RED}Last 20 lines of error log:${NC}"
        tail -20 "$log_file" || echo "No log output available"
        echo
    fi
}

# Run each test module
for test_module in "${INTEGRATION_TESTS[@]}"; do
    run_test_module "$test_module"
    
    # Brief pause between tests
    sleep 2
    
    # Clean up between tests
    pkill -f "landro-daemon.*test" || true
    rm -rf /tmp/landropic-test* /tmp/landro-* 2>/dev/null || true
done

echo
print_status "Integration test run complete!"

# Step 4: Report results
echo -e "${BLUE}📊 Test Results Summary${NC}"
echo "===================="

if [ ${#PASSED_TESTS[@]} -gt 0 ]; then
    echo -e "${GREEN}✅ Passed tests (${#PASSED_TESTS[@]}):${NC}"
    for test in "${PASSED_TESTS[@]}"; do
        echo "  - $test"
    done
    echo
fi

if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
    echo -e "${RED}❌ Failed tests (${#FAILED_TESTS[@]}):${NC}"
    for test in "${FAILED_TESTS[@]}"; do
        echo "  - $test"
    done
    echo
fi

# Final status
TOTAL_TESTS=$((${#PASSED_TESTS[@]} + ${#FAILED_TESTS[@]}))
echo "Total tests: $TOTAL_TESTS"
echo "Passed: ${#PASSED_TESTS[@]}"
echo "Failed: ${#FAILED_TESTS[@]}"

if [ ${#FAILED_TESTS[@]} -eq 0 ]; then
    print_success "All integration tests passed! 🎉"
    exit 0
else
    print_error "Some integration tests failed."
    print_warning "Check individual test logs in /tmp/landropic_test_*.log for details"
    exit 1
fi