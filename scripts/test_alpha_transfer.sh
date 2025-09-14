#!/bin/bash

# Alpha file transfer test script
set -e

echo "Starting Landropic Alpha File Transfer Test"
echo "=========================================="

# Set up test environment
TEST_DIR="/tmp/landropic_alpha_test"
TEST_FILE="$TEST_DIR/test_file.txt"
DAEMON_LOG="$TEST_DIR/daemon.log"
RECEIVED_FILE="$HOME/LandropicSync/received_file.tmp"

# Clean up any previous test
rm -rf "$TEST_DIR" "$HOME/LandropicSync/received_file.tmp"
mkdir -p "$TEST_DIR"

# Create test file
echo "Hello Landropic Alpha! File transfer test at $(date)" > "$TEST_FILE"
echo "Test file created: $TEST_FILE"
echo "Content: $(cat "$TEST_FILE")"
echo

# Start daemon in background
echo "Starting daemon..."
cd /Users/rexliu/landropic
RUST_LOG=info ./target/debug/landro-daemon --port 19876 --storage "$TEST_DIR/.landropic" > "$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!

# Give daemon time to start up
echo "Waiting for daemon to start (PID: $DAEMON_PID)..."
sleep 3

# Check if daemon is running
if ! kill -0 $DAEMON_PID 2>/dev/null; then
    echo "ERROR: Daemon failed to start"
    cat "$DAEMON_LOG"
    exit 1
fi

echo "Daemon started successfully"
echo

# Attempt file transfer
echo "Attempting file transfer..."
timeout 10 ./target/debug/file-transfer 127.0.0.1:19876 "$TEST_FILE" || {
    echo "ERROR: File transfer failed or timed out"
    echo "Daemon log:"
    cat "$DAEMON_LOG"
    kill $DAEMON_PID 2>/dev/null
    exit 1
}

echo "File transfer command completed"
echo

# Check if file was received
sleep 1
if [ -f "$RECEIVED_FILE" ]; then
    echo "SUCCESS: File received at $RECEIVED_FILE"
    echo "Original content: $(cat "$TEST_FILE")"
    echo "Received content: $(cat "$RECEIVED_FILE")"
    
    # Verify content matches
    if diff "$TEST_FILE" "$RECEIVED_FILE" > /dev/null; then
        echo "SUCCESS: File content matches exactly!"
    else
        echo "ERROR: File content mismatch"
        echo "Diff:"
        diff "$TEST_FILE" "$RECEIVED_FILE" || true
        kill $DAEMON_PID 2>/dev/null
        exit 1
    fi
else
    echo "ERROR: File not received at expected location: $RECEIVED_FILE"
    echo "Checking if LandropicSync directory was created..."
    ls -la "$HOME/LandropicSync/" 2>/dev/null || echo "Directory does not exist"
    echo "Daemon log:"
    cat "$DAEMON_LOG"
    kill $DAEMON_PID 2>/dev/null
    exit 1
fi

# Clean up
echo "Cleaning up..."
kill $DAEMON_PID 2>/dev/null
wait $DAEMON_PID 2>/dev/null

echo
echo "=========================================="
echo "Alpha File Transfer Test COMPLETED!"
echo "=========================================="
echo
echo "Summary:"
echo "- Daemon started successfully"
echo "- File transfer completed within timeout"
echo "- File received and content verified"
echo "- Test PASSED ✅"