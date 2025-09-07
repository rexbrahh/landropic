#!/bin/bash
set -e

# Build Validation Script for Landropic
# Senior Engineer Build Validation Infrastructure
# 
# This script provides comprehensive build validation with detailed reporting
# Designed for both CI/CD and local development use

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
NC='\033[0m' # No Color

# Configuration
BUILD_REPORT_FILE="${BUILD_REPORT_FILE:-build-report.json}"
VERBOSE="${VERBOSE:-false}"
FAIL_FAST="${FAIL_FAST:-true}"
CHECK_WARNINGS="${CHECK_WARNINGS:-true}"

# Workspace crates (from Cargo.toml)
WORKSPACE_CRATES=(
    "landro-proto"
    "landro-crypto" 
    "landro-quic"
    "landro-chunker"
    "landro-cas"
    "landro-index"
    "landro-daemon"
    "landro-cli"
)

# Global counters
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0
BUILD_WARNINGS=0
BUILD_ERRORS=0

# Utility functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
    ((BUILD_WARNINGS++))
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
    ((BUILD_ERRORS++))
}

log_step() {
    echo -e "${PURPLE}[STEP]${NC} $1"
}

# JSON report functions
init_report() {
    cat > "$BUILD_REPORT_FILE" << EOF
{
  "timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "git_commit": "$(git rev-parse HEAD 2>/dev/null || echo 'unknown')",
  "git_branch": "$(git branch --show-current 2>/dev/null || echo 'unknown')",
  "rust_version": "$(rustc --version 2>/dev/null || echo 'unavailable')",
  "cargo_version": "$(cargo --version 2>/dev/null || echo 'unavailable')",
  "build_results": {
    "overall_status": "running",
    "workspace_build": {},
    "crate_builds": {},
    "test_results": {},
    "lint_results": {},
    "warnings": 0,
    "errors": 0
  },
  "performance": {
    "build_time": 0,
    "test_time": 0
  }
}
EOF
}

update_report() {
    local key="$1"
    local value="$2"
    local temp_file=$(mktemp)
    
    jq "$key = $value" "$BUILD_REPORT_FILE" > "$temp_file" && mv "$temp_file" "$BUILD_REPORT_FILE"
}

finalize_report() {
    local overall_status="success"
    if [[ $BUILD_ERRORS -gt 0 ]]; then
        overall_status="failed"
    elif [[ $BUILD_WARNINGS -gt 0 ]]; then
        overall_status="warnings"
    fi
    
    local temp_file=$(mktemp)
    jq ".build_results.overall_status = \"$overall_status\" | 
        .build_results.warnings = $BUILD_WARNINGS | 
        .build_results.errors = $BUILD_ERRORS |
        .performance.total_time = $(date +%s) - .performance.start_time" \
        "$BUILD_REPORT_FILE" > "$temp_file" && mv "$temp_file" "$BUILD_REPORT_FILE"
}

# Pre-flight checks
check_prerequisites() {
    log_step "Checking prerequisites..."
    
    # Check Rust installation
    if ! command -v cargo &> /dev/null; then
        log_error "Cargo not found. Please install Rust: https://rustup.rs/"
        return 1
    fi
    
    if ! command -v rustc &> /dev/null; then
        log_error "Rust compiler not found."
        return 1
    fi
    
    # Check protobuf compiler
    if ! command -v protoc &> /dev/null; then
        log_warning "protoc not found. Some builds may fail."
        log_info "Install with: brew install protobuf (macOS) or apt-get install protobuf-compiler (Ubuntu)"
    fi
    
    # Check jq for JSON reports
    if ! command -v jq &> /dev/null; then
        log_warning "jq not found. JSON reports will be limited."
    fi
    
    log_success "Prerequisites check completed"
    return 0
}

# Clean build environment
clean_build() {
    log_step "Cleaning build environment..."
    
    if cargo clean; then
        log_success "Build environment cleaned"
    else
        log_warning "Failed to clean build environment"
    fi
}

# Build workspace
build_workspace() {
    log_step "Building entire workspace..."
    
    local start_time=$(date +%s)
    local build_output
    local build_status=0
    
    # Capture both stdout and stderr
    if build_output=$(cargo build --workspace --all-targets 2>&1); then
        log_success "Workspace build completed successfully"
        update_report '.build_results.workspace_build.status' '"success"'
    else
        build_status=$?
        log_error "Workspace build failed"
        log_error "Build output:"
        echo "$build_output" | while IFS= read -r line; do
            echo "  $line"
        done
        update_report '.build_results.workspace_build.status' '"failed"'
        
        if [[ "$FAIL_FAST" == "true" ]]; then
            return $build_status
        fi
    fi
    
    # Check for warnings in build output
    local warning_count=$(echo "$build_output" | grep -c "warning:" || true)
    if [[ $warning_count -gt 0 ]]; then
        log_warning "Found $warning_count warnings in workspace build"
        BUILD_WARNINGS=$((BUILD_WARNINGS + warning_count))
        
        if [[ "$CHECK_WARNINGS" == "true" && "$VERBOSE" == "true" ]]; then
            echo "$build_output" | grep -A2 -B1 "warning:" || true
        fi
    fi
    
    local end_time=$(date +%s)
    local build_time=$((end_time - start_time))
    update_report '.performance.build_time' "$build_time"
    
    return $build_status
}

# Build individual crates
build_individual_crates() {
    log_step "Building individual crates..."
    
    for crate in "${WORKSPACE_CRATES[@]}"; do
        log_info "Building crate: $crate"
        
        local crate_output
        local crate_status=0
        
        if crate_output=$(cargo build -p "$crate" --all-targets 2>&1); then
            log_success "✓ $crate built successfully"
            update_report ".build_results.crate_builds.\"$crate\".status" '"success"'
        else
            crate_status=$?
            log_error "✗ $crate build failed"
            update_report ".build_results.crate_builds.\"$crate\".status" '"failed"'
            
            if [[ "$VERBOSE" == "true" ]]; then
                echo "$crate_output" | head -20
            fi
            
            if [[ "$FAIL_FAST" == "true" ]]; then
                return $crate_status
            fi
        fi
        
        # Check crate-specific warnings
        local crate_warnings=$(echo "$crate_output" | grep -c "warning:" || true)
        if [[ $crate_warnings -gt 0 ]]; then
            log_warning "$crate has $crate_warnings warnings"
            update_report ".build_results.crate_builds.\"$crate\".warnings" "$crate_warnings"
        fi
    done
}

# Run release build validation
build_release() {
    log_step "Running release build validation..."
    
    local release_output
    local release_status=0
    
    if release_output=$(cargo build --workspace --release 2>&1); then
        log_success "Release build completed successfully"
        update_report '.build_results.release_build.status' '"success"'
    else
        release_status=$?
        log_error "Release build failed"
        update_report '.build_results.release_build.status' '"failed"'
        
        if [[ "$VERBOSE" == "true" ]]; then
            echo "$release_output"
        fi
        
        return $release_status
    fi
}

# Run tests
run_tests() {
    log_step "Running comprehensive test suite..."
    
    local start_time=$(date +%s)
    local test_output
    local test_status=0
    
    # Run workspace tests with detailed output
    if test_output=$(cargo test --workspace --all-targets 2>&1); then
        log_success "All tests passed"
        update_report '.build_results.test_results.status' '"success"'
        
        # Extract test statistics
        local test_count=$(echo "$test_output" | grep -o "test result: ok\. [0-9]* passed" | grep -o "[0-9]*" || echo "0")
        TOTAL_TESTS=$test_count
        PASSED_TESTS=$test_count
        
        update_report '.build_results.test_results.total' "$TOTAL_TESTS"
        update_report '.build_results.test_results.passed' "$PASSED_TESTS"
        update_report '.build_results.test_results.failed' "0"
        
    else
        test_status=$?
        log_error "Tests failed"
        update_report '.build_results.test_results.status' '"failed"'
        
        # Try to extract failure information
        local failed_count=$(echo "$test_output" | grep -o "[0-9]* failed" | grep -o "[0-9]*" || echo "0")
        FAILED_TESTS=$failed_count
        
        if [[ "$VERBOSE" == "true" ]]; then
            echo "$test_output"
        fi
        
        update_report '.build_results.test_results.failed' "$FAILED_TESTS"
    fi
    
    local end_time=$(date +%s)
    local test_time=$((end_time - start_time))
    update_report '.performance.test_time' "$test_time"
    
    return $test_status
}

# Run linting
run_linting() {
    log_step "Running code quality checks..."
    
    # Clippy analysis
    local clippy_output
    local clippy_status=0
    
    if clippy_output=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1); then
        log_success "Clippy analysis passed"
        update_report '.build_results.lint_results.clippy.status' '"success"'
    else
        clippy_status=$?
        log_error "Clippy analysis failed"
        update_report '.build_results.lint_results.clippy.status' '"failed"'
        
        if [[ "$VERBOSE" == "true" ]]; then
            echo "$clippy_output"
        fi
    fi
    
    # Format check
    local fmt_status=0
    if cargo fmt --all -- --check > /dev/null 2>&1; then
        log_success "Code formatting is correct"
        update_report '.build_results.lint_results.fmt.status' '"success"'
    else
        fmt_status=$?
        log_error "Code formatting issues found"
        log_info "Run 'cargo fmt --all' to fix formatting"
        update_report '.build_results.lint_results.fmt.status' '"failed"'
    fi
    
    return $((clippy_status + fmt_status))
}

# Generate build summary
generate_summary() {
    log_step "Generating build summary..."
    
    echo
    echo "=================================="
    echo "        BUILD VALIDATION REPORT"
    echo "=================================="
    echo
    echo "Timestamp: $(date)"
    echo "Git Branch: $(git branch --show-current 2>/dev/null || echo 'unknown')"
    echo "Git Commit: $(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
    echo
    echo "Build Results:"
    echo "  Warnings: $BUILD_WARNINGS"
    echo "  Errors: $BUILD_ERRORS"
    echo
    echo "Test Results:"
    echo "  Total: $TOTAL_TESTS"
    echo "  Passed: $PASSED_TESTS" 
    echo "  Failed: $FAILED_TESTS"
    echo
    
    if [[ $BUILD_ERRORS -eq 0 && $FAILED_TESTS -eq 0 ]]; then
        log_success "🎉 All validation checks passed!"
        if [[ $BUILD_WARNINGS -gt 0 ]]; then
            log_warning "⚠️  Build completed with $BUILD_WARNINGS warnings"
        fi
        return 0
    else
        log_error "❌ Validation failed with $BUILD_ERRORS errors and $FAILED_TESTS test failures"
        return 1
    fi
}

# Cleanup function
cleanup() {
    local exit_code=$?
    finalize_report
    
    if [[ "$VERBOSE" == "true" && -f "$BUILD_REPORT_FILE" ]]; then
        log_info "Full build report:"
        cat "$BUILD_REPORT_FILE" | jq '.' 2>/dev/null || cat "$BUILD_REPORT_FILE"
    fi
    
    exit $exit_code
}

# Main execution
main() {
    local start_time=$(date +%s)
    
    # Set up trap for cleanup
    trap cleanup EXIT
    
    # Initialize report
    init_report
    jq ".performance.start_time = $start_time" "$BUILD_REPORT_FILE" > /tmp/build_report.tmp && mv /tmp/build_report.tmp "$BUILD_REPORT_FILE"
    
    echo "🚀 Landropic Build Validation Started"
    echo "Report will be saved to: $BUILD_REPORT_FILE"
    echo
    
    # Run validation steps
    check_prerequisites || exit 1
    
    # Optional: Clean build for fresh validation
    if [[ "${CLEAN_BUILD:-false}" == "true" ]]; then
        clean_build
    fi
    
    build_workspace || exit 1
    build_individual_crates || exit 1
    build_release || exit 1
    run_tests || exit 1
    run_linting || exit 1
    
    generate_summary
}

# Handle command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --verbose|-v)
            VERBOSE="true"
            shift
            ;;
        --no-fail-fast)
            FAIL_FAST="false"
            shift
            ;;
        --ignore-warnings)
            CHECK_WARNINGS="false"
            shift
            ;;
        --clean)
            CLEAN_BUILD="true"
            shift
            ;;
        --report-file)
            BUILD_REPORT_FILE="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo "Options:"
            echo "  --verbose, -v          Enable verbose output"
            echo "  --no-fail-fast         Continue on first failure"
            echo "  --ignore-warnings      Don't fail on warnings"
            echo "  --clean               Clean before building"
            echo "  --report-file FILE     Specify report file path"
            echo "  --help, -h            Show this help"
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Run main function
main "$@"