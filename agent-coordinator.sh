#!/bin/bash
# Multi-Agent Development Coordinator
# Usage: ./scripts/agent-coordinator.sh [command]

set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$WORKSPACE_ROOT"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if all agents can work simultaneously
check_agent_readiness() {
    log_info "Checking agent development readiness..."
    
    # Check if workspace builds
    if cargo check --workspace --quiet; then
        log_success "Workspace builds successfully"
    else
        log_error "Workspace has build errors - fix before multi-agent development"
        return 1
    fi
    
    # Check for conflicts in current changes
    if git status --porcelain | grep -q "^UU"; then
        log_error "Git merge conflicts detected - resolve before continuing"
        return 1
    fi
    
    # Verify test suite baseline
    log_info "Running baseline test suite..."
    if cargo test --workspace --quiet --profile test-fast; then
        log_success "All tests pass - agents can proceed"
    else
        log_warning "Some tests failing - agents should be aware of current state"
    fi
    
    return 0
}

# Create agent-specific branches
create_agent_branches() {
    local current_branch=$(git branch --show-current)
    log_info "Current branch: $current_branch"
    
    # Create feature branches for each agent domain
    local branches=(
        "feature/transport-layer"
        "feature/storage-layer" 
        "feature/sync-engine"
        "feature/security-layer"
    )
    
    for branch in "${branches[@]}"; do
        if git show-ref --verify --quiet "refs/heads/$branch"; then
            log_warning "Branch $branch already exists"
        else
            git checkout -b "$branch" "$current_branch"
            log_success "Created branch: $branch"
        fi
    done
    
    git checkout "$current_branch"
    log_info "Returned to $current_branch"
}

# Run comprehensive test matrix
run_test_matrix() {
    log_info "Running comprehensive test matrix..."
    
    local crates=(
        "landro-proto"
        "landro-crypto" 
        "landro-quic"
        "landro-chunker"
        "landro-cas"
        "landro-index"
        "landro-daemon"
        "landro-cli"
    )
    
    local failed_crates=()
    
    for crate in "${crates[@]}"; do
        log_info "Testing $crate..."
        if cargo test -p "$crate" --profile test-fast --quiet; then
            log_success "$crate tests passed"
        else
            log_error "$crate tests failed"
            failed_crates+=("$crate")
        fi
    done
    
    if [ ${#failed_crates[@]} -eq 0 ]; then
        log_success "All crate tests passed!"
    else
        log_error "Failed crates: ${failed_crates[*]}"
        return 1
    fi
}

# Continuous integration check for multi-agent work
ci_check() {
    log_info "Running CI checks suitable for multi-agent development..."
    
    # Fast checks first
    log_info "Running clippy..."
    cargo clippy --workspace --all-targets --quiet
    
    log_info "Checking formatting..."
    cargo fmt --all -- --check
    
    log_info "Running fast test suite..."
    cargo test --workspace --profile test-fast --quiet
    
    log_success "All CI checks passed!"
}

# Monitor builds across all crates
monitor_workspace() {
    log_info "Starting workspace-wide monitoring..."
    log_info "This will watch all crates and run tests on changes..."
    
    # Use cargo-watch to monitor the entire workspace
    cargo watch \
        --clear \
        --exec "check --workspace" \
        --exec "test --workspace --profile test-fast" \
        --exec "clippy --workspace --all-targets"
}

# Show current workspace status for agents
status() {
    log_info "=== Landropic Multi-Agent Status ==="
    
    echo -e "\n${BLUE}Git Status:${NC}"
    git status --short
    
    echo -e "\n${BLUE}Current Branch:${NC}"
    git branch --show-current
    
    echo -e "\n${BLUE}Recent Commits:${NC}"
    git log --oneline -5
    
    echo -e "\n${BLUE}Workspace Build Status:${NC}"
    if cargo check --workspace --quiet 2>/dev/null; then
        log_success "Workspace builds cleanly"
    else
        log_error "Workspace has build issues"
    fi
    
    echo -e "\n${BLUE}Available Agent Commands:${NC}"
    echo "  FEATURE AGENTS:"
    echo "    just dev-crypto    # Crypto agent development loop"
    echo "    just dev-quic      # QUIC agent development loop"  
    echo "    just dev-storage   # Storage agent development loop"
    echo "    just dev-sync      # Sync agent development loop"
    echo ""
    echo "  QA/VALIDATION AGENT:"
    echo "    just qa-full       # Comprehensive testing workflow"
    echo "    just qa-monitor    # Continuous test monitoring"
    echo "    just qa-security   # Security audit and checks"
    echo ""
    echo "  CODE QUALITY AGENT:"
    echo "    just quality-sweep # Full quality improvement sweep"
    echo "    just quality-check # Quality analysis and reporting"
    echo "    just fix-clippy    # Fix all clippy warnings"
    echo ""
    echo "  CI/CD AGENT:"
    echo "    just cicd-orchestrate  # Full pipeline orchestration"
    echo "    just cicd-deploy-ready # Deployment readiness check"
    echo "    just cicd-release-prep # Release preparation workflow"
    echo ""
    echo "  COORDINATION:"
    echo "    just agent-status  # Show all agent status"
}

# Main command dispatch
case "${1:-status}" in
    "check")
        check_agent_readiness
        ;;
    "branches")
        create_agent_branches
        ;;
    "test-matrix")
        run_test_matrix
        ;;
    "ci")
        ci_check
        ;;
    "monitor")
        monitor_workspace
        ;;
    "status")
        status
        ;;
    *)
        echo "Usage: $0 [check|branches|test-matrix|ci|monitor|status]"
        echo ""
        echo "Commands:"
        echo "  check       - Verify workspace is ready for multi-agent development"
        echo "  branches    - Create feature branches for each agent domain"
        echo "  test-matrix - Run comprehensive test matrix across all crates"
        echo "  ci          - Run CI checks optimized for multi-agent workflow"
        echo "  monitor     - Start workspace-wide file monitoring"
        echo "  status      - Show current development status"
        ;;
esac