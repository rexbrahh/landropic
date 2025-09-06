# Multi-Agent Development Coordination

This document outlines the coordination protocols for the 6-agent development team working on landropic.

## Agent Roster

### Feature Development Agents
- **AGENT-1-TRANSPORT**: ✅ COMPLETE - QUIC transport layer, mDNS discovery, connection management
- **AGENT-2-STORAGE**: ✅ COMPLETE - Content-addressed storage (CAS), chunking, database operations  
- **AGENT-3-CRYPTO**: ✅ COMPLETE - Identity management, TLS certificates, security implementation

### Quality & Operations Agents
- **AGENT-4-QA-VALIDATOR**: Testing, integration validation, cross-platform verification
- **AGENT-5-CODE-QUALITY**: Bug fixes, refactoring, code quality, performance optimization
- **AGENT-6-CICD**: Pipeline automation, deployment readiness, release coordination

## Collaboration Workflows

### Primary Collaborations

#### AGENT-6-CICD ↔ AGENT-4-QA-VALIDATOR
- **Pipeline Integration**: CI/CD orchestrates comprehensive test suites
- **Quality Gates**: QA validates before deployment milestones
- **Test Automation**: Shared responsibility for automated testing infrastructure

```bash
# CI/CD initiates QA validation
just cicd-run-tests     # Calls QA test suites
just qa-full           # QA comprehensive validation
just cicd-quality-gates # Combined quality assessment
```

#### AGENT-6-CICD ↔ AGENT-5-CODE-QUALITY  
- **Pre-commit Hooks**: Quality standards enforcement
- **Code Standards**: Automated formatting and linting integration
- **Technical Debt**: Coordinated refactoring during release cycles

```bash
# Quality-CI coordination workflow
just quality-sweep      # Quality improvements
just cicd-quality-gates # CI/CD validates changes
just cicd-deploy-ready  # Final deployment check
```

#### AGENT-6-CICD ↔ ALL AGENTS
- **Release Coordination**: Ensures all domains are deployment-ready
- **Integration Testing**: Cross-domain functionality validation
- **Status Monitoring**: Tracks agent progress for release planning

### Agent Status Coordination

```bash
# Check overall system status
just agent-status

# Validate all agents are stable before major operations  
just cicd-validate-agents

# Full system health check
just cicd-orchestrate
```

## Workflow Examples

### Feature Development Cycle
1. **Feature agents** (1-3) implement domain-specific features
2. **QA agent** (4) validates feature functionality 
3. **Quality agent** (5) ensures code standards compliance
4. **CI/CD agent** (6) orchestrates integration and deployment readiness

### Bug Fix Cycle
1. **Quality agent** (5) identifies and fixes issues
2. **QA agent** (4) validates fixes with regression testing
3. **CI/CD agent** (6) ensures fix doesn't break deployment pipeline
4. **Feature agents** coordinate if fixes affect domain interfaces

### Release Cycle
1. **CI/CD agent** (6) initiates release preparation
2. **All agents** ensure their domains are stable
3. **QA agent** (4) performs comprehensive validation
4. **Quality agent** (5) completes final code quality sweep
5. **CI/CD agent** (6) validates deployment readiness

## Communication Protocols

### Status Updates
- Update `.agent-status` when starting/completing major work
- Use status values: `idle`, `active`, `testing`, `committing`, `monitoring`, `blocked`

### Dependency Management
- Feature agents coordinate through integration tests
- Quality agents validate cross-domain changes
- CI/CD agent monitors for deployment blockers

### Conflict Resolution
- Use `just cicd-validate-agents` to identify blocked agents
- Coordinate through status file for async communication
- CI/CD agent mediates deployment conflicts

## Command Reference

### Agent-Specific Commands
```bash
# Feature Development
just dev-crypto    # Agent 1
just dev-quic      # Agent 2  
just dev-storage   # Agent 3

# Quality & Operations
just qa-full           # Agent 4
just quality-sweep     # Agent 5
just cicd-orchestrate  # Agent 6
```

### Cross-Agent Workflows
```bash
# Full system validation
just cicd-deploy-ready

# Quality + QA coordination
just quality-check && just qa-security

# Release preparation
just cicd-release-prep
```