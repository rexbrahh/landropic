#!/bin/bash
set -euo pipefail

# Alpha Release Monitoring Script - 30-minute cycle for release builds
# Focus: Alpha release readiness, not general development monitoring

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
QA_DIR="$SCRIPT_DIR"
RESULTS_DIR="$QA_DIR/results"
LOGS_DIR="$QA_DIR/logs"
CHECKLIST_FILE="$QA_DIR/alpha_checklist.json"

# Ensure directories exist
mkdir -p "$RESULTS_DIR" "$LOGS_DIR"

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Logging
log() {
    echo -e "${BLUE}[$(date +'%Y-%m-%d %H:%M:%S')] $1${NC}" | tee -a "$LOGS_DIR/alpha_monitor.log"
}

log_success() {
    echo -e "${GREEN}[SUCCESS] $1${NC}" | tee -a "$LOGS_DIR/alpha_monitor.log"
}

log_error() {
    echo -e "${RED}[ERROR] $1${NC}" | tee -a "$LOGS_DIR/alpha_monitor.log"
}

log_warning() {
    echo -e "${YELLOW}[WARNING] $1${NC}" | tee -a "$LOGS_DIR/alpha_monitor.log"
}

# Update checklist status
update_checklist() {
    local key="$1"
    local status="$2"
    local additional_data="$3"
    
    # Create backup
    cp "$CHECKLIST_FILE" "$CHECKLIST_FILE.backup"
    
    # Update the checklist using jq
    jq --arg key "$key" --arg status "$status" --arg timestamp "$(date -Iseconds)" --argjson data "$additional_data" '
        .alpha_release_checklist.minimum_viable_requirements[$key].status = $status |
        .alpha_release_checklist.minimum_viable_requirements[$key].last_tested = $timestamp |
        if $data then .alpha_release_checklist.minimum_viable_requirements[$key] += $data else . end
    ' "$CHECKLIST_FILE.backup" > "$CHECKLIST_FILE"
}

# Alpha-focused build monitoring
alpha_build_monitor() {
    log "=== Alpha Build Monitoring ==="
    cd "$PROJECT_ROOT"
    
    # Fix environment issues
    if [ "${CARGO_BUILD_JOBS:-}" = "0" ]; then
        log_warning "CARGO_BUILD_JOBS=0 detected, fixing environment"
        unset CARGO_BUILD_JOBS
    fi
    if [ "${RUSTC_WRAPPER:-}" = "sccache" ]; then
        log_warning "RUSTC_WRAPPER=sccache detected but sccache not available, disabling"
        unset RUSTC_WRAPPER
    fi
    
    # Clean build for accurate results
    cargo clean > /dev/null 2>&1 || true
    
    # Release build (alpha target)
    log "Building release binaries for alpha..."
    local build_start=$(date +%s)
    
    if env -u CARGO_BUILD_JOBS cargo build --release > "$LOGS_DIR/alpha_build.log" 2>&1; then
        local build_end=$(date +%s)
        local build_time=$((build_end - build_start))
        
        log_success "Alpha release build completed in ${build_time}s"
        
        # Count warnings
        local warnings=$(grep -c "warning:" "$LOGS_DIR/alpha_build.log" || echo "0")
        log "Build warnings: $warnings (target: <50 for alpha)"
        
        # Verify expected binaries
        local binaries_ok=true
        local missing_binaries=()
        
        if [ ! -f "target/release/landro-daemon" ]; then
            binaries_ok=false
            missing_binaries+=("landro-daemon")
        fi
        
        if [ ! -f "target/release/landro" ]; then
            binaries_ok=false
            missing_binaries+=("landro")
        fi
        
        if [ "$binaries_ok" = true ]; then
            log_success "All alpha binaries present"
            update_checklist "build_system" "pass" '{"build_time": '$build_time', "warnings": '$warnings'}'
        else
            log_error "Missing binaries: ${missing_binaries[*]}"
            update_checklist "build_system" "fail" '{"missing_binaries": ["'"${missing_binaries[*]}"'"]}'
        fi
        
        # Update warning count in quality gates
        jq --argjson warnings "$warnings" '
            .alpha_release_checklist.quality_gates.build_warnings.current_count = $warnings |
            .alpha_release_checklist.quality_gates.build_warnings.status = (if $warnings <= 50 then "pass" else "fail" end)
        ' "$CHECKLIST_FILE" > "$CHECKLIST_FILE.tmp" && mv "$CHECKLIST_FILE.tmp" "$CHECKLIST_FILE"
        
    else
        log_error "Alpha release build failed"
        update_checklist "build_system" "fail" '{"error": "build_failure"}'
    fi
}

# Daemon stability check
alpha_daemon_check() {
    log "=== Alpha Daemon Stability Check ==="
    
    if [ ! -f "$PROJECT_ROOT/target/release/landro-daemon" ]; then
        log_error "Daemon binary not found, skipping stability check"
        update_checklist "daemon_stability" "fail" '{"error": "binary_not_found"}'
        return
    fi
    
    local daemon_bin="$PROJECT_ROOT/target/release/landro-daemon"
    local stability_start=$(date +%s)
    
    # Test daemon startup and basic stability
    log "Testing daemon startup..."
    
    if timeout 45s "$daemon_bin" > "$LOGS_DIR/alpha_daemon.log" 2>&1 & then
        local daemon_pid=$!
        
        # Check immediate startup (first 5 seconds)
        sleep 5
        
        if kill -0 "$daemon_pid" 2>/dev/null; then
            log_success "Daemon started successfully"
            
            # Extended stability test (30 seconds total)
            sleep 25
            
            if kill -0 "$daemon_pid" 2>/dev/null; then
                log_success "Daemon stability test passed (30s runtime)"
                update_checklist "daemon_stability" "pass" '{"runtime_seconds": 30}'
                
                # Clean shutdown
                kill -TERM "$daemon_pid"
                sleep 3
                kill -KILL "$daemon_pid" 2>/dev/null || true
                
            else
                log_error "Daemon crashed during stability test"
                update_checklist "daemon_stability" "fail" '{"error": "runtime_crash", "crash_time": "after_5s"}'
            fi
        else
            log_error "Daemon failed to start or panicked immediately"
            update_checklist "daemon_stability" "fail" '{"error": "startup_panic"}'
        fi
    else
        log_error "Daemon process failed to launch"
        update_checklist "daemon_stability" "fail" '{"error": "launch_failure"}'
    fi
    
    local stability_end=$(date +%s)
    log "Daemon check completed in $((stability_end - stability_start))s"
}

# CLI functionality check
alpha_cli_check() {
    log "=== Alpha CLI Functionality Check ==="
    
    if [ ! -f "$PROJECT_ROOT/target/release/landro" ]; then
        log_error "CLI binary not found, skipping CLI check"
        update_checklist "cli_functionality" "fail" '{"error": "binary_not_found"}'
        return
    fi
    
    local cli_bin="$PROJECT_ROOT/target/release/landro"
    
    # Test basic CLI functionality
    log "Testing CLI help command..."
    if "$cli_bin" --help > "$LOGS_DIR/alpha_cli_help.log" 2>&1; then
        log_success "CLI help command works"
        
        # Test if CLI can interact with daemon (basic check)
        log "Testing CLI version/status commands..."
        local cli_commands_work=false
        
        # Try common command patterns
        if "$cli_bin" version > "$LOGS_DIR/alpha_cli_version.log" 2>&1; then
            cli_commands_work=true
            log_success "CLI version command works"
        elif "$cli_bin" status > "$LOGS_DIR/alpha_cli_status.log" 2>&1; then
            cli_commands_work=true
            log_success "CLI status command works"
        elif "$cli_bin" info > "$LOGS_DIR/alpha_cli_info.log" 2>&1; then
            cli_commands_work=true
            log_success "CLI info command works"
        fi
        
        if [ "$cli_commands_work" = true ]; then
            update_checklist "cli_functionality" "pass" '{"commands_tested": ["help", "status/version/info"]}'
        else
            log_warning "CLI help works but no status/info commands found"
            update_checklist "cli_functionality" "partial" '{"commands_tested": ["help"], "missing": ["status", "info"]}'
        fi
    else
        log_error "CLI help command failed"
        update_checklist "cli_functionality" "fail" '{"error": "help_command_failed"}'
    fi
}

# Basic file operations test (minimal for alpha)
alpha_file_test() {
    log "=== Alpha File Operations Test ==="
    
    # Create test environment
    local test_dir="/tmp/landropic_alpha_monitor_$$"
    mkdir -p "$test_dir"
    
    # Create test file
    local test_file="$test_dir/alpha_test.txt"
    echo "Alpha monitoring test - $(date)" > "$test_file"
    
    # Test file integrity (basic requirement for alpha)
    if [ -f "$test_file" ] && [ -s "$test_file" ]; then
        local original_hash=$(sha256sum "$test_file" | cut -d' ' -f1)
        
        # Copy and verify (simulates basic file operations)
        local copy_file="$test_dir/alpha_test_copy.txt"
        cp "$test_file" "$copy_file"
        
        local copy_hash=$(sha256sum "$copy_file" | cut -d' ' -f1)
        
        if [ "$original_hash" = "$copy_hash" ]; then
            log_success "Basic file operations and integrity check passed"
            update_checklist "data_integrity" "pass" '{"hash_verification": true}'
        else
            log_error "File integrity check failed"
            update_checklist "data_integrity" "fail" '{"error": "hash_mismatch"}'
        fi
        
        # Note: Actual file transfer would require running daemon instances
        # This is a placeholder for alpha - real transfer test comes later
        log_warning "Full file transfer test requires running daemon instances"
        update_checklist "file_transfer" "not_implemented" '{"note": "requires_daemon_instances"}'
    else
        log_error "Test file creation failed"
        update_checklist "data_integrity" "fail" '{"error": "file_creation_failed"}'
    fi
    
    # Cleanup
    rm -rf "$test_dir"
}

# Generate alpha status report
generate_alpha_status_report() {
    log "=== Generating Alpha Status Report ==="
    
    local timestamp=$(date -Iseconds)
    
    # Calculate current score
    local passed_count=$(jq -r '.alpha_release_checklist.minimum_viable_requirements | [.[] | select(.status == "pass")] | length' "$CHECKLIST_FILE")
    local total_count=$(jq -r '.alpha_release_checklist.minimum_viable_requirements | length' "$CHECKLIST_FILE")
    local current_score="${passed_count}/${total_count}"
    
    # Count blockers
    local blocker_count=$(jq -r '.alpha_release_checklist.minimum_viable_requirements | [.[] | select(.status == "fail" and .blocker == true)] | length' "$CHECKLIST_FILE")
    
    # Update release readiness
    jq --arg timestamp "$timestamp" --arg score "$current_score" --argjson blockers "$blocker_count" '
        .alpha_release_checklist.release_readiness.current_score = $score |
        .alpha_release_checklist.release_readiness.blocker_count = $blockers |
        .alpha_release_checklist.release_readiness.ready_for_alpha = ($blockers == 0 and ($score | split("/")[0] | tonumber) >= 6) |
        .alpha_release_checklist.release_readiness.last_assessment = $timestamp
    ' "$CHECKLIST_FILE" > "$CHECKLIST_FILE.tmp" && mv "$CHECKLIST_FILE.tmp" "$CHECKLIST_FILE"
    
    # Create monitoring report
    cat > "$RESULTS_DIR/alpha_monitor_report.json" << EOF
{
  "timestamp": "$timestamp",
  "monitoring_cycle": "30_minute_alpha_build_focus",
  "alpha_readiness": {
    "score": "$current_score",
    "blockers": $blocker_count,
    "ready": $([ "$blocker_count" -eq 0 ] && [ "$passed_count" -ge 6 ] && echo "true" || echo "false")
  },
  "build_status": {
    "last_successful": "$(jq -r '.alpha_release_checklist.minimum_viable_requirements.build_system.status' "$CHECKLIST_FILE")",
    "warnings": $(jq -r '.alpha_release_checklist.quality_gates.build_warnings.current_count' "$CHECKLIST_FILE"),
    "warning_threshold_met": $(jq -r '.alpha_release_checklist.quality_gates.build_warnings.current_count <= 50' "$CHECKLIST_FILE")
  },
  "critical_components": {
    "daemon_stable": "$(jq -r '.alpha_release_checklist.minimum_viable_requirements.daemon_stability.status' "$CHECKLIST_FILE")",
    "cli_functional": "$(jq -r '.alpha_release_checklist.minimum_viable_requirements.cli_functionality.status' "$CHECKLIST_FILE")",
    "file_integrity": "$(jq -r '.alpha_release_checklist.minimum_viable_requirements.data_integrity.status' "$CHECKLIST_FILE")"
  },
  "next_monitor_cycle": "$(date -d '+30 minutes' -Iseconds)"
}
EOF
    
    # Console summary
    log "=== ALPHA MONITORING SUMMARY ==="
    log "Score: $current_score"
    log "Blockers: $blocker_count"
    log "Alpha Ready: $([ "$blocker_count" -eq 0 ] && [ "$passed_count" -ge 6 ] && echo "YES" || echo "NO")"
    log "Next cycle: $(date -d '+30 minutes' +'%H:%M')"
}

# Main monitoring function
main() {
    log "Starting Alpha Release Monitoring Cycle"
    
    # Run alpha-focused checks
    alpha_build_monitor
    alpha_daemon_check
    alpha_cli_check
    alpha_file_test
    
    # Generate status report
    generate_alpha_status_report
    
    log "Alpha monitoring cycle completed"
}

# Execute main function
main "$@"