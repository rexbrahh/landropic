#!/bin/bash
# QA Continuous Quality Monitor
# Runs every 10 minutes to check build status and detect regressions

QUALITY_LOG="/Users/rexliu/landropic/quality-log.txt"
BUILD_LOG="/Users/rexliu/landropic/build-monitor.log"

echo "=== QUALITY MONITOR STARTING $(date) ===" | tee -a $QUALITY_LOG

while true; do
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    
    # Test compilation
    echo "=== BUILD CHECK $timestamp ===" >> $BUILD_LOG
    RUSTC_WRAPPER="" compile_result=$(cargo build --workspace -j 2 2>&1)
    compile_status=$?
    echo "Exit code: $compile_status" >> $BUILD_LOG
    echo "$compile_result" >> $BUILD_LOG
    echo "---" >> $BUILD_LOG
    
    # Test CLI
    cli_result=$(timeout 5 ./target/debug/landropic --help 2>&1)
    cli_status=$?
    
    # Test daemon (expect panic but not hang)
    daemon_result=$(timeout 10 ./target/debug/landro-daemon --help 2>&1)
    daemon_status=$?
    
    # Count warnings
    warning_count=$(echo "$compile_result" | grep -c "warning")
    
    # Log results
    echo "$timestamp: Build=$compile_status CLI=$cli_status Daemon=$daemon_status Warnings=$warning_count" >> $QUALITY_LOG
    
    # Alert on regressions
    if [ $compile_status -ne 0 ]; then
        npx claude-flow@alpha hooks notify --message "QA ALERT: Build broken again!" 2>/dev/null || echo "QA ALERT: Build broken!" >> $QUALITY_LOG
    fi
    
    if [ $cli_status -ne 0 ]; then
        npx claude-flow@alpha hooks notify --message "QA ALERT: CLI broken!" 2>/dev/null || echo "QA ALERT: CLI broken!" >> $QUALITY_LOG
    fi
    
    # Check for hanging (timeout should be 124 for hanging, we expect panic=101)
    if [ $daemon_status -eq 124 ]; then
        npx claude-flow@alpha hooks notify --message "QA ALERT: Daemon hanging again!" 2>/dev/null || echo "QA ALERT: Daemon hanging!" >> $QUALITY_LOG
    fi
    
    sleep 600  # Every 10 minutes
done