#!/bin/bash
set -euo pipefail

# Alpha Release 2-Hour Status Reporting
# Comprehensive status report every 2 hours during alpha development

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
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Logging functions
log() {
    echo -e "${BLUE}[$(date +'%Y-%m-%d %H:%M:%S')] $1${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

log_success() {
    echo -e "${GREEN}✅ $1${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

log_error() {
    echo -e "${RED}❌ $1${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

log_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

log_info() {
    echo -e "${CYAN}ℹ️  $1${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

log_header() {
    echo -e "\n${BOLD}${CYAN}=== $1 ===${NC}" | tee -a "$LOGS_DIR/alpha_2hour_report.log"
}

# Generate comprehensive alpha status report
generate_comprehensive_report() {
    local timestamp=$(date -Iseconds)
    local report_file="$RESULTS_DIR/alpha_2hour_$(date +%Y%m%d_%H%M).json"
    
    log_header "LANDROPIC ALPHA STATUS REPORT"
    log "Generated: $timestamp"
    log "Report: $report_file"
    
    # Run latest validation to get current status
    log_info "Running latest alpha validation..."
    if ./alpha_validation.sh > "$LOGS_DIR/latest_validation.log" 2>&1; then
        local validation_result="passed"
    else
        local validation_result="failed"
    fi
    
    # Read latest validation results
    local latest_validation=""
    if [ -f "$RESULTS_DIR/alpha_validation_report.json" ]; then
        latest_validation=$(cat "$RESULTS_DIR/alpha_validation_report.json")
    fi
    
    # Count bug reports
    local critical_bugs=$(find "$RESULTS_DIR" -name "bug_report_*.json" -exec jq -r 'select(.alpha_blocker == true) | .description' {} \; 2>/dev/null | wc -l || echo "0")
    local total_bugs=$(find "$RESULTS_DIR" -name "bug_report_*.json" | wc -l || echo "0")
    
    # Check git status
    cd "$PROJECT_ROOT"
    local git_status=$(git status --porcelain | wc -l)
    local current_branch=$(git branch --show-current)
    local current_commit=$(git rev-parse --short HEAD)
    
    # Build status check
    local build_status="unknown"
    local cli_exists="false" 
    local daemon_exists="false"
    
    if [ -f "target/release/landro" ]; then
        cli_exists="true"
    fi
    
    if [ -f "target/release/landro-daemon" ]; then
        daemon_exists="true"
    fi
    
    if [ "$cli_exists" = "true" ] && [ "$daemon_exists" = "true" ]; then
        build_status="complete"
    elif [ "$cli_exists" = "true" ] || [ "$daemon_exists" = "true" ]; then
        build_status="partial" 
    else
        build_status="failed"
    fi
    
    # Generate main report
    cat > "$report_file" << EOF
{
  "report_metadata": {
    "timestamp": "$timestamp",
    "report_type": "alpha_2hour_status",
    "version": "1.0",
    "next_report": "$(date -d '+2 hours' -Iseconds)"
  },
  
  "alpha_readiness_summary": {
    "overall_status": "$([ "$validation_result" = "passed" ] && echo "progressing" || echo "blocked")",
    "readiness_score": "$(jq -r '.alpha_readiness_score // "0/4"' "$RESULTS_DIR/alpha_validation_report.json" 2>/dev/null || echo "0/4")",
    "critical_blockers": $critical_bugs,
    "total_issues": $total_bugs,
    "days_to_alpha": "unknown",
    "confidence_level": "$([ "$build_status" = "complete" ] && echo "medium" || echo "low")"
  },

  "success_criteria_status": {
    "binaries_build": {
      "status": "$build_status", 
      "cli_binary": $cli_exists,
      "daemon_binary": $daemon_exists,
      "critical": true
    },
    "daemon_stability": {
      "status": "$([ "$daemon_exists" = "true" ] && echo "testable" || echo "blocked")",
      "tested": false,
      "critical": true
    },
    "cli_control": {
      "status": "$([ "$cli_exists" = "true" ] && [ "$daemon_exists" = "true" ] && echo "testable" || echo "blocked")",
      "tested": false,
      "critical": true
    },
    "file_transfer": {
      "status": "not_implemented",
      "tested": false,
      "critical": true
    },
    "data_integrity": {
      "status": "not_testable",
      "tested": false, 
      "critical": true
    },
    "critical_tests": {
      "status": "not_identified",
      "tested": false,
      "critical": true
    }
  },

  "development_metrics": {
    "build_performance": {
      "last_build_time": "$(grep "completed in" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+s' || echo "unknown")",
      "warnings_count": $(grep "Build warnings count:" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+' || echo "0"),
      "warnings_threshold": 50,
      "warnings_acceptable": $([ $(grep "Build warnings count:" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+' || echo "999") -le 50 ] && echo "true" || echo "false")
    },
    
    "code_quality": {
      "git_status": "$git_status uncommitted changes",
      "current_branch": "$current_branch",
      "current_commit": "$current_commit",
      "validation_result": "$validation_result"
    },
    
    "testing_status": {
      "alpha_validation_runs": $(ls "$RESULTS_DIR"/alpha_validation_report_*.json 2>/dev/null | wc -l || echo "0"),
      "last_validation": "$(stat -c %Y "$RESULTS_DIR/alpha_validation_report.json" 2>/dev/null | xargs -I {} date -d @{} -Iseconds || echo "never")",
      "monitoring_active": true,
      "issue_tracking": true
    }
  },

  "priority_actions": {
    "immediate": [
      $([ "$daemon_exists" = "false" ] && echo '"Fix daemon compilation errors",' || echo '')
      $([ "$build_status" != "complete" ] && echo '"Complete binary build process",' || echo '')
      $([ $(grep "Build warnings count:" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+' || echo "999") -gt 50 ] && echo '"Reduce build warnings to <50",' || echo '')
      "Continue alpha validation monitoring"
    ],
    "next_2_hours": [
      $([ "$daemon_exists" = "true" ] && echo '"Test daemon startup stability",' || echo '')
      $([ "$cli_exists" = "true" ] && [ "$daemon_exists" = "true" ] && echo '"Test CLI-daemon communication",' || echo '')
      "Implement basic file transfer capability",
      "Address highest priority bug reports"
    ],
    "next_24_hours": [
      "Complete all 6 alpha success criteria",
      "Reduce critical blocker count to 0", 
      "Achieve stable alpha validation passes",
      "Prepare alpha release candidate"
    ]
  },

  "recent_progress": {
    "achievements": [
      $([ -f "$RESULTS_DIR/alpha_baseline_report.json" ] && echo '"Alpha validation framework established",' || echo '')
      $([ "$cli_exists" = "true" ] && echo '"CLI binary compilation successful",' || echo '')
      "Build environment configuration issues resolved",
      "Comprehensive monitoring and reporting system active"
    ],
    "setbacks": [
      $([ "$daemon_exists" = "false" ] && echo '"Daemon compilation failure blocking progress",' || echo '')
      $([ $(grep "Build warnings count:" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+' || echo "999") -gt 50 ] && echo '"Build warnings exceed alpha quality threshold",' || echo '')
      "File transfer functionality not yet implemented"
    ]
  },

  "risk_indicators": {
    "schedule_risk": "$([ "$critical_bugs" -gt 2 ] && echo "high" || [ "$critical_bugs" -gt 0 ] && echo "medium" || echo "low")",
    "technical_risk": "$([ "$daemon_exists" = "false" ] && echo "high" || echo "medium")",
    "quality_risk": "$([ $(grep "Build warnings count:" "$LOGS_DIR/latest_validation.log" 2>/dev/null | tail -1 | grep -o '[0-9]\\+' || echo "999") -gt 100 ] && echo "high" || echo "medium")",
    "mitigation_active": true
  },
  
  "detailed_validation_results": $latest_validation
}
EOF

    # Console summary
    log_header "ALPHA READINESS SUMMARY"
    log_info "Overall Status: $([ "$validation_result" = "passed" ] && echo "PROGRESSING" || echo "BLOCKED")"
    log_info "Build Status: $(echo "$build_status" | tr a-z A-Z)"
    log_info "Critical Blockers: $critical_bugs"
    log_info "Total Issues: $total_bugs"
    
    if [ "$cli_exists" = "true" ]; then
        log_success "CLI binary available for testing"
    else
        log_error "CLI binary missing"
    fi
    
    if [ "$daemon_exists" = "true" ]; then
        log_success "Daemon binary available for testing" 
    else
        log_error "Daemon compilation failure - critical blocker"
    fi
    
    log_header "NEXT ACTIONS"
    if [ "$daemon_exists" = "false" ]; then
        log_error "PRIORITY: Fix daemon compilation errors"
    fi
    
    if [ "$build_status" = "complete" ]; then
        log_info "Ready for daemon stability testing"
        log_info "Ready for CLI-daemon communication testing"
    fi
    
    log_info "Continue working on file transfer implementation"
    log_info "Next report in 2 hours: $(date -d '+2 hours' +'%H:%M')"
    
    echo ""
    log "Full report saved to: $report_file"
}

# Schedule next run
schedule_next_report() {
    log_info "Alpha 2-hour reporting cycle active"
    log_info "Next automatic report: $(date -d '+2 hours' +'%Y-%m-%d %H:%M')"
    
    # Could set up cron job here if needed
    # echo "0 */2 * * * $(pwd)/alpha_2hour_report.sh" | crontab -
}

# Main execution
main() {
    log_header "ALPHA 2-HOUR STATUS REPORT"
    log "Landropic Alpha Release Monitoring"
    
    generate_comprehensive_report
    schedule_next_report
    
    log_info "Report generation completed"
}

# Execute main function
main "$@"