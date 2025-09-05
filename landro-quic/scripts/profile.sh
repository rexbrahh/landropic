#!/bin/bash
# Performance profiling script for landro-quic

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Landropic QUIC Performance Profiling${NC}"
echo "========================================="

# Check for required tools
check_tool() {
    if ! command -v $1 &> /dev/null; then
        echo -e "${RED}Error: $1 is not installed${NC}"
        echo "Please install $1 to continue"
        exit 1
    fi
}

# Profile selection
profile_type=${1:-all}

case $profile_type in
    flamegraph)
        echo -e "${YELLOW}Running Flamegraph profiling...${NC}"
        check_tool "cargo-flamegraph"
        
        # Build with debug symbols
        cargo build --release
        
        # Run with flamegraph
        cargo flamegraph --bench throughput_bench -- --bench
        echo -e "${GREEN}Flamegraph saved to flamegraph.svg${NC}"
        ;;
        
    perf)
        echo -e "${YELLOW}Running perf profiling...${NC}"
        check_tool "perf"
        
        # Build with debug symbols
        cargo build --release
        
        # Record performance data
        perf record -g --call-graph=dwarf cargo bench --bench throughput_bench
        perf report
        ;;
        
    valgrind)
        echo -e "${YELLOW}Running Valgrind memory profiling...${NC}"
        check_tool "valgrind"
        
        # Build debug version
        cargo build
        
        # Run with valgrind
        valgrind --tool=massif --massif-out-file=massif.out \
            ./target/debug/deps/throughput_bench-*
        
        echo -e "${GREEN}Memory profile saved to massif.out${NC}"
        echo "View with: ms_print massif.out"
        ;;
        
    bench)
        echo -e "${YELLOW}Running performance benchmarks...${NC}"
        
        # Run throughput benchmarks
        echo -e "\n${GREEN}Throughput Benchmarks:${NC}"
        cargo bench --bench throughput_bench
        
        # Run integration benchmarks
        echo -e "\n${GREEN}Integration Benchmarks:${NC}"
        cargo bench --bench integration_bench
        
        echo -e "\n${GREEN}Benchmark results saved to target/criterion/${NC}"
        ;;
        
    quick)
        echo -e "${YELLOW}Running quick performance test...${NC}"
        
        # Build in release mode
        cargo build --release --examples
        
        # Run a quick test
        echo -e "${GREEN}Running quick transfer test...${NC}"
        cargo test --release test_transfer_performance --features bench
        ;;
        
    all)
        echo -e "${YELLOW}Running all profiling tools...${NC}"
        
        # Run benchmarks first
        $0 bench
        
        # Generate flamegraph if available
        if command -v cargo-flamegraph &> /dev/null; then
            $0 flamegraph
        fi
        
        # Quick performance test
        $0 quick
        ;;
        
    *)
        echo "Usage: $0 [flamegraph|perf|valgrind|bench|quick|all]"
        echo ""
        echo "Options:"
        echo "  flamegraph - Generate a flamegraph of CPU usage"
        echo "  perf       - Use Linux perf for detailed profiling"
        echo "  valgrind   - Memory profiling with Valgrind"
        echo "  bench      - Run all benchmarks"
        echo "  quick      - Quick performance test"
        echo "  all        - Run all available profiling"
        exit 1
        ;;
esac

echo -e "\n${GREEN}Profiling complete!${NC}"

# Generate summary report
if [ -f "target/criterion/throughput/single_stream/base/estimates.json" ]; then
    echo -e "\n${YELLOW}Performance Summary:${NC}"
    echo "-------------------"
    
    # Parse and display key metrics (simplified)
    echo "Throughput results available in: target/criterion/"
    echo "View detailed HTML report: open target/criterion/report/index.html"
fi

# Suggestions for optimization
echo -e "\n${YELLOW}Optimization Suggestions:${NC}"
echo "1. Check flamegraph.svg for CPU hotspots"
echo "2. Review target/criterion/report/index.html for benchmark trends"
echo "3. Use 'cargo build --profile=release-with-debug' for better symbols"
echo "4. Consider using 'cargo-instruments' on macOS for detailed profiling"