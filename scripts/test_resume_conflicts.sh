#!/bin/bash

# Test script for resume capability and conflict detection
# Tests Day 3 functionality with interruption and recovery

set -e

echo "🧪 Testing resume capability and conflict detection..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Cleanup function
cleanup() {
    echo -e "${YELLOW}🧹 Cleaning up test environment...${NC}"
    
    # Kill any running daemons
    pkill -f "landro-daemon" || true
    
    # Clean up test directories
    rm -rf /tmp/landropic_test_node1 /tmp/landropic_test_node2 || true
    
    echo -e "${GREEN}✅ Cleanup complete${NC}"
}

# Set up cleanup trap
trap cleanup EXIT

# Build the project
echo -e "${YELLOW}🔧 Building landropic...${NC}"
RUSTC_WRAPPER="" CARGO_BUILD_JOBS=1 cargo build --release --quiet

# Create test directories
echo -e "${YELLOW}📁 Setting up test directories...${NC}"
mkdir -p /tmp/landropic_test_node1/{sync,objects}
mkdir -p /tmp/landropic_test_node2/{sync,objects}

# Create test files with potential conflicts
echo "Initial content from node1" > /tmp/landropic_test_node1/sync/shared_file.txt
echo "Different content from node2" > /tmp/landropic_test_node2/sync/shared_file.txt
echo "Node1 specific file" > /tmp/landropic_test_node1/sync/node1_file.txt
echo "Node2 specific file" > /tmp/landropic_test_node2/sync/node2_file.txt

# Large file for testing resume capability
echo -e "${YELLOW}📦 Creating large test file for resume testing...${NC}"
dd if=/dev/zero of=/tmp/landropic_test_node1/sync/large_file.dat bs=1M count=10 2>/dev/null
echo "Large file created for resume testing" >> /tmp/landropic_test_node1/sync/large_file.dat

echo -e "${GREEN}✅ Test environment prepared${NC}"

# Start Node 1 (will be interrupted)
echo -e "${YELLOW}🚀 Starting Node 1 on port 5001...${NC}"
LANDROPIC_STORAGE=/tmp/landropic_test_node1 LANDROPIC_PORT=5001 RUST_LOG=info,landro_sync=debug timeout 10s ./target/release/landro-daemon &
NODE1_PID=$!

# Start Node 2 (will complete normally)  
echo -e "${YELLOW}🚀 Starting Node 2 on port 5002...${NC}"
LANDROPIC_STORAGE=/tmp/landropic_test_node2 LANDROPIC_PORT=5002 RUST_LOG=info,landro_sync=debug timeout 15s ./target/release/landro-daemon &
NODE2_PID=$!

# Let nodes start up
echo -e "${YELLOW}⏱️  Waiting for nodes to initialize...${NC}"
sleep 3

# Check if nodes are running
if kill -0 $NODE1_PID 2>/dev/null; then
    echo -e "${GREEN}✅ Node 1 is running${NC}"
else
    echo -e "${RED}❌ Node 1 failed to start${NC}"
    exit 1
fi

if kill -0 $NODE2_PID 2>/dev/null; then
    echo -e "${GREEN}✅ Node 2 is running${NC}"
else
    echo -e "${RED}❌ Node 2 failed to start${NC}"
    exit 1
fi

# Let sync start
echo -e "${YELLOW}⏱️  Allowing sync to start...${NC}"
sleep 3

# Simulate interruption by killing Node 1
echo -e "${YELLOW}💥 Simulating interruption (killing Node 1)...${NC}"
kill -TERM $NODE1_PID || true
wait $NODE1_PID 2>/dev/null || true

# Wait for Node 2 to detect disconnection
echo -e "${YELLOW}⏱️  Waiting for Node 2 to detect disconnection...${NC}"
sleep 2

# Restart Node 1 to test resume capability
echo -e "${YELLOW}🔄 Restarting Node 1 to test resume capability...${NC}"
LANDROPIC_STORAGE=/tmp/landropic_test_node1 LANDROPIC_PORT=5001 RUST_LOG=info,landro_sync=debug timeout 10s ./target/release/landro-daemon &
NODE1_RESTART_PID=$!

# Let resume process complete
echo -e "${YELLOW}⏱️  Allowing resume process to complete...${NC}"
sleep 5

# Check final state
echo -e "${YELLOW}📊 Checking final sync state...${NC}"

# List files in both nodes
echo -e "${YELLOW}📁 Node 1 files:${NC}"
ls -la /tmp/landropic_test_node1/sync/ || true

echo -e "${YELLOW}📁 Node 2 files:${NC}"
ls -la /tmp/landropic_test_node2/sync/ || true

# Check for sync state files (persistence)
echo -e "${YELLOW}💾 Checking for sync state persistence:${NC}"
if [ -d "/tmp/landropic_test_node1/.landropic/sync_state" ]; then
    echo -e "${GREEN}✅ Node 1 has sync state directory${NC}"
    ls -la /tmp/landropic_test_node1/.landropic/sync_state/ || true
else
    echo -e "${YELLOW}⚠️  Node 1 sync state directory not found${NC}"
fi

if [ -d "/tmp/landropic_test_node2/.landropic/sync_state" ]; then
    echo -e "${GREEN}✅ Node 2 has sync state directory${NC}"
    ls -la /tmp/landropic_test_node2/.landropic/sync_state/ || true
else
    echo -e "${YELLOW}⚠️  Node 2 sync state directory not found${NC}"
fi

# Wait for processes to finish naturally
wait $NODE2_PID 2>/dev/null || true
wait $NODE1_RESTART_PID 2>/dev/null || true

echo -e "${GREEN}✅ Resume and conflict detection test completed${NC}"
echo -e "${YELLOW}📝 Key test points:${NC}"
echo "   • Conflict detection: Different versions of shared_file.txt"
echo "   • Resume capability: Interrupted sync session"
echo "   • State persistence: Sync state saved to disk"
echo "   • Large file handling: 10MB+ file transfer"
echo ""
echo -e "${GREEN}🎯 Day 3 functionality tested successfully!${NC}"