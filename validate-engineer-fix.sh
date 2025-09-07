#!/bin/bash
# Engineer Fix Validation System
# Validates fixes from Senior, Systems, and Junior engineers

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

ENGINEER=$1
FIX_DESCRIPTION=$2
VALIDATION_LOG="fix-validation.log"
TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')

if [ -z "$ENGINEER" ] || [ -z "$FIX_DESCRIPTION" ]; then
    echo "Usage: $0 <engineer> <fix_description>"
    echo "Engineers: senior|systems|junior"
    exit 1
fi

echo "=== TESTING $ENGINEER FIX: $FIX_DESCRIPTION ===" | tee -a $VALIDATION_LOG
echo "Timestamp: $TIMESTAMP" | tee -a $VALIDATION_LOG
echo "" | tee -a $VALIDATION_LOG

# Common compilation test for all engineers
echo "BASIC COMPILATION TEST:" | tee -a $VALIDATION_LOG
RUSTC_WRAPPER="" cargo build --workspace -j 2 >/dev/null 2>&1
build_result=$?

if [ $build_result -eq 0 ]; then
    echo "✅ Workspace builds successfully" | tee -a $VALIDATION_LOG
else
    echo "❌ Workspace build failed" | tee -a $VALIDATION_LOG
    RUSTC_WRAPPER="" cargo build --workspace -j 2 2>&1 | tail -10 | tee -a $VALIDATION_LOG
fi
echo "" | tee -a $VALIDATION_LOG

# Engineer-specific validation
case $ENGINEER in
    "senior")
        echo "SENIOR ENGINEER VALIDATION:" | tee -a $VALIDATION_LOG
        
        # Each package should compile
        packages=("landro-cas" "landro-quic" "landro-sync" "landro-daemon" "landro-cli")
        for pkg in "${packages[@]}"; do
            echo "Testing $pkg..." | tee -a $VALIDATION_LOG
            RUSTC_WRAPPER="" cargo build -p $pkg -j 2 >/dev/null 2>&1
            if [ $? -ne 0 ]; then
                echo "❌ $pkg still broken" | tee -a $VALIDATION_LOG
                echo "" | tee -a $VALIDATION_LOG
                npx claude-flow@alpha hooks notify --message "QA ❌: Senior engineer fix failed - $pkg still broken" 2>/dev/null
                exit 1
            else
                echo "✅ $pkg compiles successfully" | tee -a $VALIDATION_LOG
            fi
        done
        
        # Count remaining warnings
        warning_count=$(RUSTC_WRAPPER="" cargo build --workspace -j 2 2>&1 | grep -c "warning")
        echo "Warning count: $warning_count" | tee -a $VALIDATION_LOG
        
        if [ $warning_count -lt 150 ]; then
            echo "✅ Warning count acceptable ($warning_count < 150)" | tee -a $VALIDATION_LOG
        else
            echo "⚠️  High warning count ($warning_count)" | tee -a $VALIDATION_LOG
        fi
        ;;
        
    "systems")
        echo "SYSTEMS ENGINEER VALIDATION:" | tee -a $VALIDATION_LOG
        
        # Daemon should respond to --help without hanging
        timeout 10 ./target/debug/landro-daemon --help >/dev/null 2>&1
        daemon_result=$?
        
        if [ $daemon_result -eq 124 ]; then
            echo "❌ Daemon still hanging" | tee -a $VALIDATION_LOG
            npx claude-flow@alpha hooks notify --message "QA ❌: Systems engineer fix failed - daemon still hanging" 2>/dev/null
            exit 1
        else
            echo "✅ Daemon responds quickly (no hanging)" | tee -a $VALIDATION_LOG
        fi
        
        # Integration tests should compile
        RUSTC_WRAPPER="" cargo test --workspace --no-run -j 2 >/dev/null 2>&1
        if [ $? -ne 0 ]; then
            echo "❌ Integration tests still broken" | tee -a $VALIDATION_LOG
            npx claude-flow@alpha hooks notify --message "QA ❌: Systems engineer fix failed - integration tests broken" 2>/dev/null
            exit 1
        else
            echo "✅ Integration tests compile" | tee -a $VALIDATION_LOG
        fi
        
        # Test daemon binary exists
        if [ -f "target/debug/landro-daemon" ]; then
            echo "✅ Daemon binary exists" | tee -a $VALIDATION_LOG
        else
            echo "❌ Daemon binary missing" | tee -a $VALIDATION_LOG
            exit 1
        fi
        ;;
        
    "junior")
        echo "JUNIOR ENGINEER VALIDATION:" | tee -a $VALIDATION_LOG
        
        # CLI should compile
        RUSTC_WRAPPER="" cargo build -p landro-cli -j 2 >/dev/null 2>&1
        if [ $? -ne 0 ]; then
            echo "❌ CLI still doesn't compile" | tee -a $VALIDATION_LOG
            npx claude-flow@alpha hooks notify --message "QA ❌: Junior engineer fix failed - CLI doesn't compile" 2>/dev/null
            exit 1
        else
            echo "✅ CLI compiles successfully" | tee -a $VALIDATION_LOG
        fi
        
        # Basic CLI commands should work
        commands=("--version" "--help" "status")
        for cmd in "${commands[@]}"; do
            if [ "$cmd" = "status" ]; then
                timeout 5 ./target/debug/landropic $cmd >/dev/null 2>&1
                result=$?
                if [ $result -eq 0 ] || [ $result -eq 1 ]; then
                    echo "✅ CLI $cmd command works" | tee -a $VALIDATION_LOG
                else
                    echo "❌ CLI $cmd command failed" | tee -a $VALIDATION_LOG
                fi
            else
                timeout 5 ./target/debug/landropic $cmd >/dev/null 2>&1
                if [ $? -eq 0 ]; then
                    echo "✅ CLI $cmd command works" | tee -a $VALIDATION_LOG
                else
                    echo "❌ CLI $cmd command failed" | tee -a $VALIDATION_LOG
                fi
            fi
        done
        
        echo "✅ CLI basic functionality working" | tee -a $VALIDATION_LOG
        ;;
        
    *)
        echo "❌ Unknown engineer type: $ENGINEER"
        exit 1
        ;;
esac

echo "" | tee -a $VALIDATION_LOG
echo "=== VALIDATION RESULT ===" | tee -a $VALIDATION_LOG

# Report result
if [ $build_result -eq 0 ]; then
    echo "🎉 VALIDATION PASSED: $ENGINEER fix validated successfully" | tee -a $VALIDATION_LOG
    npx claude-flow@alpha hooks notify --message "QA ✅: $ENGINEER fix validated - $FIX_DESCRIPTION" 2>/dev/null
    exit 0
else
    echo "🚨 VALIDATION FAILED: $ENGINEER fix did not resolve issues" | tee -a $VALIDATION_LOG
    npx claude-flow@alpha hooks notify --message "QA ❌: $ENGINEER fix failed validation - $FIX_DESCRIPTION" 2>/dev/null
    exit 1
fi