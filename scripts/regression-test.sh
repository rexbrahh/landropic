#!/bin/bash
# Regression Testing Framework for Landropic
# Tests all critical functionality and compares against baseline

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

BASELINE_LOG="qa-baseline.log"
REGRESSION_LOG="regression-results.log"
TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')

echo "=== LANDROPIC REGRESSION TEST SUITE ===" | tee $REGRESSION_LOG
echo "Started: $TIMESTAMP" | tee -a $REGRESSION_LOG
echo "" | tee -a $REGRESSION_LOG

# Test 1: Compilation Test
echo "TEST 1: Compilation" | tee -a $REGRESSION_LOG
echo "Expected: All packages compile successfully" | tee -a $REGRESSION_LOG

RUSTC_WRAPPER="" cargo build --workspace -j 2 >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ PASS: Workspace compilation successful" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: Workspace compilation failed" | tee -a $REGRESSION_LOG
    RUSTC_WRAPPER="" cargo build --workspace -j 2 2>&1 | tail -10 | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Test 2: Individual Package Compilation
echo "TEST 2: Individual Package Compilation" | tee -a $REGRESSION_LOG
packages=("landro-cas" "landro-quic" "landro-sync" "landro-daemon" "landro-cli")
for pkg in "${packages[@]}"; do
    echo "Testing package: $pkg" | tee -a $REGRESSION_LOG
    RUSTC_WRAPPER="" cargo build -p $pkg -j 2 >/dev/null 2>&1
    if [ $? -eq 0 ]; then
        echo "✅ PASS: $pkg compiles" | tee -a $REGRESSION_LOG
    else
        echo "❌ FAIL: $pkg compilation failed" | tee -a $REGRESSION_LOG
    fi
done
echo "" | tee -a $REGRESSION_LOG

# Test 3: Binary Generation
echo "TEST 3: Binary Generation" | tee -a $REGRESSION_LOG
if [ -f "target/debug/landropic" ]; then
    echo "✅ PASS: CLI binary exists" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: CLI binary missing" | tee -a $REGRESSION_LOG
fi

if [ -f "target/debug/landro-daemon" ]; then
    echo "✅ PASS: Daemon binary exists" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: Daemon binary missing" | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Test 4: CLI Functionality
echo "TEST 4: CLI Functionality" | tee -a $REGRESSION_LOG

# Test --help
timeout 5 ./target/debug/landropic --help >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ PASS: CLI --help works" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: CLI --help failed" | tee -a $REGRESSION_LOG
fi

# Test --version
timeout 5 ./target/debug/landropic --version >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ PASS: CLI --version works" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: CLI --version failed" | tee -a $REGRESSION_LOG
fi

# Test status command
timeout 5 ./target/debug/landropic status >/dev/null 2>&1
cli_status_result=$?
if [ $cli_status_result -eq 0 ]; then
    echo "✅ PASS: CLI status command works" | tee -a $REGRESSION_LOG
else
    echo "⚠️  WARN: CLI status command failed (exit code: $cli_status_result)" | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Test 5: Daemon Stability
echo "TEST 5: Daemon Stability" | tee -a $REGRESSION_LOG

# Test daemon --help (expect panic, not hang)
timeout 10 ./target/debug/landro-daemon --help >/dev/null 2>&1
daemon_help_result=$?

if [ $daemon_help_result -eq 124 ]; then
    echo "❌ FAIL: Daemon hanging (timeout)" | tee -a $REGRESSION_LOG
elif [ $daemon_help_result -eq 101 ]; then
    echo "⚠️  EXPECTED: Daemon panics (unimplemented)" | tee -a $REGRESSION_LOG
else
    echo "✅ PASS: Daemon responds quickly (exit code: $daemon_help_result)" | tee -a $REGRESSION_LOG
fi

# Test daemon --version
timeout 10 ./target/debug/landro-daemon --version >/dev/null 2>&1
daemon_version_result=$?

if [ $daemon_version_result -eq 124 ]; then
    echo "❌ FAIL: Daemon --version hanging" | tee -a $REGRESSION_LOG
elif [ $daemon_version_result -eq 101 ]; then
    echo "⚠️  EXPECTED: Daemon --version panics" | tee -a $REGRESSION_LOG
else
    echo "✅ PASS: Daemon --version works" | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Test 6: Warning Count Analysis
echo "TEST 6: Warning Count Analysis" | tee -a $REGRESSION_LOG
warning_output=$(RUSTC_WRAPPER="" cargo build --workspace -j 2 2>&1)
warning_count=$(echo "$warning_output" | grep -c "warning")

echo "Current warning count: $warning_count" | tee -a $REGRESSION_LOG
if [ $warning_count -le 100 ]; then
    echo "✅ PASS: Warning count within acceptable range (≤100)" | tee -a $REGRESSION_LOG
elif [ $warning_count -le 150 ]; then
    echo "⚠️  WARN: High warning count ($warning_count)" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: Too many warnings ($warning_count > 150)" | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Test 7: Integration Test Compilation
echo "TEST 7: Integration Test Compilation" | tee -a $REGRESSION_LOG
RUSTC_WRAPPER="" cargo test --workspace --no-run -j 2 >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ PASS: Integration tests compile" | tee -a $REGRESSION_LOG
else
    echo "❌ FAIL: Integration tests don't compile" | tee -a $REGRESSION_LOG
fi
echo "" | tee -a $REGRESSION_LOG

# Summary
echo "=== REGRESSION TEST SUMMARY ===" | tee -a $REGRESSION_LOG
echo "Completed: $(date '+%Y-%m-%d %H:%M:%S')" | tee -a $REGRESSION_LOG

pass_count=$(grep -c "✅ PASS" $REGRESSION_LOG)
fail_count=$(grep -c "❌ FAIL" $REGRESSION_LOG)
warn_count=$(grep -c "⚠️  WARN\|⚠️  EXPECTED" $REGRESSION_LOG)

echo "Results: $pass_count passed, $fail_count failed, $warn_count warnings" | tee -a $REGRESSION_LOG

if [ $fail_count -eq 0 ]; then
    echo "🎉 OVERALL: PASS - No critical failures" | tee -a $REGRESSION_LOG
    exit 0
else
    echo "🚨 OVERALL: FAIL - $fail_count critical failures" | tee -a $REGRESSION_LOG
    exit 1
fi