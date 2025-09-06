# Multi-Agent Development Setup

## 5-Agent Coordination System

### Current Agent Assignments

**Feature Development Agents (1-3):**
- **Agent 1**: Transport Layer (QUIC, networking) - `just dev-quic`
- **Agent 2**: Storage Systems (CAS, chunking) - `just dev-storage`  
- **Agent 3**: Crypto/Security (TLS, identity) - `just dev-crypto`

**Quality Assurance Agents (4-5):**
- **Agent 4**: QA/Testing/Validation - `just qa-full`
- **Agent 5**: Code Quality/Bug Fixes - `just quality-sweep`

## Agent 4 (QA/Testing) Setup

### Responsibilities
- Comprehensive testing across all crates
- Integration validation
- Security auditing
- Performance regression detection
- Cross-platform compatibility
- CI/CD automation

### Key Commands
```bash
# Daily workflow
just qa-monitor          # Continuous test monitoring
just qa-full             # Comprehensive test suite
just qa-security         # Security audit
just qa-performance      # Performance benchmarks

# CI/CD integration
just qa-ci-automation    # Full QA automation pipeline
just local-ci           # Local CI validation
just pre-push           # Pre-push validation
```

### Setup Instructions
1. **Install testing tools:**
   ```bash
   cargo install cargo-audit cargo-deny cargo-tarpaulin cargo-nextest
   ```

2. **Update status when starting work:**
   Edit `.agent-status`: Set status to "active", add current work description

3. **Monitoring workflow:**
   - Run `just qa-monitor` in background
   - Validate after feature agents complete work
   - Report issues via status updates

## Agent 5 (Code Quality) Setup

### Responsibilities  
- Code quality improvements
- Bug fixing and refactoring
- Performance optimization
- Dependency management
- Code complexity analysis
- Automated formatting/linting

### Key Commands
```bash
# Daily workflow
just quality-sweep       # Full quality improvement
just fix-clippy         # Fix clippy warnings
just fix-fmt           # Apply formatting
just quality-check     # Quality analysis

# CI/CD integration
just quality-ci-automation  # Quality automation pipeline
just dead-code-removal     # Remove unused code
just quality-complexity    # Complexity analysis
```

### Setup Instructions
1. **Install quality tools:**
   ```bash
   cargo install cargo-udeps cargo-machete
   ```

2. **Update status when starting work:**
   Edit `.agent-status`: Set status to "active", add current work description

3. **Quality workflow:**
   - Run `just quality-sweep` regularly
   - Monitor clippy warnings
   - Proactively refactor complex code

## CI/CD Integration

### GitHub Actions Pipeline
The CI/CD pipeline automatically runs:
- **Multi-platform builds** (Ubuntu, macOS, Windows)
- **QA validation** (Agent 4 automation)
- **Code quality enforcement** (Agent 5 automation)  
- **Security auditing**
- **Performance benchmarking**
- **Deployment readiness checks**

### Local CI/CD Commands
```bash
# Match GitHub Actions locally
just local-ci            # Full local CI pipeline
just pre-commit          # Before committing
just pre-push           # Before pushing
just release-ready      # Release validation
```

## Agent Coordination Protocol

### Status Communication
1. **Check current status:** `just agent-status`
2. **Update your status:** Edit `.agent-status` file
3. **Check workspace health:** `./scripts/agent-coordinator.sh status`

### Status Values
- `idle`: Not currently working
- `active`: Actively developing/working
- `testing`: Running tests/validation
- `committing`: Preparing commits
- `blocked`: Waiting for dependencies
- `monitoring`: Watching for issues

### Workflow Coordination
```
Feature Agents (1-3) → Implement features
         ↓
QA Agent (4) → Validates implementation
         ↓
Quality Agent (5) → Improves code quality
         ↓
All Agents → Integration validation
```

## Best Practices

### For Feature Agents (1-3)
- Use domain-specific commands (`just dev-[domain]`)
- Update status when switching tasks
- Run `just pre-commit` before committing
- Wait for QA validation on critical changes

### For QA Agent (4)
- Monitor `.agent-status` for completed work
- Run `just qa-full` after major changes
- Use `just qa-ci-automation` for comprehensive validation
- Report issues immediately via status updates

### For Quality Agent (5)
- Run `just quality-sweep` regularly
- Fix clippy warnings proactively
- Use `just quality-ci-automation` for systematic improvements
- Focus on maintainability and performance

### Cross-Agent Communication
- **Status file**: Primary communication mechanism
- **Commit messages**: Include agent role (e.g., "QA: Fix test failures")
- **PR descriptions**: Mention affected domains
- **Issue reports**: Tag relevant agents

## Emergency Procedures

### Build Failures
1. Check `./scripts/agent-coordinator.sh check`
2. Run `just local-ci` to reproduce
3. Coordinate via status file updates
4. Feature agents pause, QA agent investigates

### Test Failures
1. QA agent identifies failing tests
2. Update status to "blocked" for affected domains
3. Quality agent assists with fixes
4. Resume after validation

### Merge Conflicts
1. Use `./scripts/agent-coordinator.sh status` to see active work
2. Coordinate resolution via status file
3. Quality agent handles complex merges
4. QA agent validates post-resolution

This setup creates a highly coordinated, efficient multi-agent development environment with built-in quality assurance and continuous integration.