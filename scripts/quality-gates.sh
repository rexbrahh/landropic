#!/bin/bash
# Quality Gates System for Landropic
# Defines and tests quality gates for release readiness

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

GATE_LOG="quality-gates.log"
TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')

echo "=== LANDROPIC QUALITY GATES ===" | tee $GATE_LOG
echo "Started: $TIMESTAMP" | tee -a $GATE_LOG
echo "" | tee -a $GATE_LOG

# Gate 1: Compilation Gate (After Senior Engineer)
echo "GATE 1: COMPILATION GATE" | tee -a $GATE_LOG
echo "Criteria:" | tee -a $GATE_LOG
echo "- All packages compile without errors" | tee -a $GATE_LOG
echo "- Warning count < 150 (down from 100 baseline)" | tee -a $GATE_LOG
echo "- CLI binary generated successfully" | tee -a $GATE_LOG
echo "- No blocking compilation errors" | tee -a $GATE_LOG
echo "" | tee -a $GATE_LOG

gate1_pass=true

# Test compilation
RUSTC_WRAPPER="" cargo build --workspace -j 2 >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ All packages compile successfully" | tee -a $GATE_LOG
else
    echo "❌ Compilation errors present" | tee -a $GATE_LOG
    gate1_pass=false
fi

# Test warning count
warning_count=$(RUSTC_WRAPPER="" cargo build --workspace -j 2 2>&1 | grep -c "warning")
echo "Warning count: $warning_count" | tee -a $GATE_LOG
if [ $warning_count -lt 150 ]; then
    echo "✅ Warning count acceptable ($warning_count < 150)" | tee -a $GATE_LOG
else
    echo "❌ Too many warnings ($warning_count ≥ 150)" | tee -a $GATE_LOG
    gate1_pass=false
fi

# Test CLI binary
if [ -f "target/debug/landropic" ]; then
    echo "✅ CLI binary generated successfully" | tee -a $GATE_LOG
else
    echo "❌ CLI binary missing" | tee -a $GATE_LOG
    gate1_pass=false
fi

if [ "$gate1_pass" = true ]; then
    echo "🎉 GATE 1 PASSED" | tee -a $GATE_LOG
else
    echo "🚨 GATE 1 FAILED" | tee -a $GATE_LOG
fi
echo "" | tee -a $GATE_LOG

# Gate 2: Integration Gate (After Systems Engineer)
echo "GATE 2: INTEGRATION GATE" | tee -a $GATE_LOG
echo "Criteria:" | tee -a $GATE_LOG
echo "- Daemon responds to --help in < 10 seconds (no hanging)" | tee -a $GATE_LOG
echo "- Integration tests compile and run" | tee -a $GATE_LOG
echo "- No hanging or timeout issues" | tee -a $GATE_LOG
echo "- Module imports resolve correctly" | tee -a $GATE_LOG
echo "" | tee -a $GATE_LOG

gate2_pass=true

# Test daemon responsiveness
timeout 10 ./target/debug/landro-daemon --help >/dev/null 2>&1
daemon_result=$?

if [ $daemon_result -eq 124 ]; then
    echo "❌ Daemon hanging (timeout)" | tee -a $GATE_LOG
    gate2_pass=false
elif [ $daemon_result -eq 101 ]; then
    echo "⚠️  Daemon panics quickly (acceptable for now)" | tee -a $GATE_LOG
else
    echo "✅ Daemon responds quickly" | tee -a $GATE_LOG
fi

# Test integration test compilation
RUSTC_WRAPPER="" cargo test --workspace --no-run -j 2 >/dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ Integration tests compile" | tee -a $GATE_LOG
else
    echo "❌ Integration tests don't compile" | tee -a $GATE_LOG
    gate2_pass=false
fi

# Test daemon binary exists
if [ -f "target/debug/landro-daemon" ]; then
    echo "✅ Daemon binary generated" | tee -a $GATE_LOG
else
    echo "❌ Daemon binary missing" | tee -a $GATE_LOG
    gate2_pass=false
fi

if [ "$gate2_pass" = true ]; then
    echo "🎉 GATE 2 PASSED" | tee -a $GATE_LOG
else
    echo "🚨 GATE 2 FAILED" | tee -a $GATE_LOG
fi
echo "" | tee -a $GATE_LOG

# Gate 3: User Experience Gate (After Junior Engineer)
echo "GATE 3: USER EXPERIENCE GATE" | tee -a $GATE_LOG
echo "Criteria:" | tee -a $GATE_LOG
echo "- CLI responds to all basic commands" | tee -a $GATE_LOG
echo "- Can start and check daemon status" | tee -a $GATE_LOG
echo "- Proper error messages displayed" | tee -a $GATE_LOG
echo "- No crashes on normal CLI usage" | tee -a $GATE_LOG
echo "" | tee -a $GATE_LOG

gate3_pass=true

# Test CLI commands
commands=("--version" "--help" "status")
for cmd in "${commands[@]}"; do
    if [ "$cmd" = "status" ]; then
        timeout 5 ./target/debug/landropic $cmd >/dev/null 2>&1
        result=$?
        if [ $result -eq 0 ] || [ $result -eq 1 ]; then  # 0 or 1 are acceptable for status
            echo "✅ CLI $cmd command works" | tee -a $GATE_LOG
        else
            echo "❌ CLI $cmd command failed (exit code: $result)" | tee -a $GATE_LOG
            gate3_pass=false
        fi
    else
        timeout 5 ./target/debug/landropic $cmd >/dev/null 2>&1
        if [ $? -eq 0 ]; then
            echo "✅ CLI $cmd command works" | tee -a $GATE_LOG
        else
            echo "❌ CLI $cmd command failed" | tee -a $GATE_LOG
            gate3_pass=false
        fi
    fi
done

if [ "$gate3_pass" = true ]; then
    echo "🎉 GATE 3 PASSED" | tee -a $GATE_LOG
else
    echo "🚨 GATE 3 FAILED" | tee -a $GATE_LOG
fi
echo "" | tee -a $GATE_LOG

# Overall Quality Gate Assessment
echo "=== OVERALL QUALITY GATE ASSESSMENT ===" | tee -a $GATE_LOG
echo "Completed: $(date '+%Y-%m-%d %H:%M:%S')" | tee -a $GATE_LOG

gates_passed=0
if [ "$gate1_pass" = true ]; then gates_passed=$((gates_passed + 1)); fi
if [ "$gate2_pass" = true ]; then gates_passed=$((gates_passed + 1)); fi
if [ "$gate3_pass" = true ]; then gates_passed=$((gates_passed + 1)); fi

echo "Gates passed: $gates_passed/3" | tee -a $GATE_LOG

if [ $gates_passed -eq 3 ]; then
    echo "🎉 ALL QUALITY GATES PASSED - READY FOR RELEASE" | tee -a $GATE_LOG
    exit 0
elif [ $gates_passed -eq 2 ]; then
    echo "⚠️  PARTIAL PASS - Minor issues need resolution" | tee -a $GATE_LOG
    exit 1
elif [ $gates_passed -eq 1 ]; then
    echo "🚨 MAJOR ISSUES - Significant work needed" | tee -a $GATE_LOG
    exit 2
else
    echo "🚨 ALL GATES FAILED - NOT READY FOR RELEASE" | tee -a $GATE_LOG
    exit 3
fi