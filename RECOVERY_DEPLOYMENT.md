# Recovery Team Deployment Instructions

## 🚨 Emergency Recovery Protocol

**Current Status**: Sprint work committed (4c652ce), tech lead has identified critical issues requiring immediate intervention.

**Mission**: Deploy specialized recovery teams to restore basic functionality within 48-72 hours.

## 🎯 Recovery Team Structure

### Task Force Alpha: Senior Engineer (P0)
- **Mission**: Fix all compilation errors (19 in CLI + module issues)
- **Prompt**: `team_prompts/RECOVERY_SENIOR_ENGINEER.md`
- **Success**: All packages compile, warnings < 20
- **Timeline**: 8 hours

### Task Force Bravo: Systems Engineer (P0) 
- **Mission**: Fix module integration, stop daemon hanging
- **Prompt**: `team_prompts/RECOVERY_SYSTEMS_ENGINEER.md`
- **Success**: Daemon responds to --help, modules integrated
- **Timeline**: 8 hours (after compilation fixed)

### Task Force Charlie: Junior Engineer (P1)
- **Mission**: Rebuild CLI with basic functionality
- **Prompt**: `team_prompts/RECOVERY_JUNIOR_ENGINEER.md`  
- **Success**: CLI start/stop/status commands work
- **Timeline**: 8 hours (after daemon works)

### Task Force Delta: QA Validator (P1)
- **Mission**: Continuous validation and quality gates
- **Prompt**: `team_prompts/RECOVERY_QA_VALIDATOR.md`
- **Success**: Quality monitoring active, regressions caught
- **Timeline**: Continuous (parallel with others)

## 📋 Deployment Checklist

### Pre-deployment (5 minutes)
- [ ] Verify current commit: `git log --oneline -1` should show `4c652ce`
- [ ] Confirm codebase state: `cargo build --workspace` should show errors
- [ ] Initialize coordination: `npx claude-flow@alpha swarm init --topology mesh`

### Phase 1: Critical Path (Hour 0-8)
- [ ] **Deploy Senior Engineer** (immediately)
- [ ] **Deploy QA Validator** (parallel with senior)
- [ ] Wait for compilation fixes before next phase

### Phase 2: Integration (Hour 8-16) 
- [ ] **Deploy Systems Engineer** (after compilation fixed)
- [ ] Continue QA validation
- [ ] Monitor daemon stability progress

### Phase 3: User Interface (Hour 16-24)
- [ ] **Deploy Junior Engineer** (after daemon working)
- [ ] Final QA validation
- [ ] Prepare for integration testing

## 🚀 Team Spawning Instructions

### For Each Engineer:

#### Step 1: Open New Claude Code Session
```bash
# New terminal window/tab
cd /Users/rexliu/landropic
```

#### Step 2: Initialize Session
```bash
# Set up coordination
npx claude-flow@alpha hooks session-restore --session-id "landropic-recovery"
export LANDROPIC_RECOVERY_ROLE="[senior|systems|junior|qa]"
```

#### Step 3: Deploy with Full Prompt

**Senior Engineer**:
```bash
# Copy ENTIRE contents of team_prompts/RECOVERY_SENIOR_ENGINEER.md
# Paste as first message to Claude in new session
```

**Systems Engineer**: 
```bash
# Wait for senior engineer to report "compilation fixed"
# Then copy team_prompts/RECOVERY_SYSTEMS_ENGINEER.md
# Paste as first message to Claude
```

**Junior Engineer**:
```bash  
# Wait for systems engineer to report "daemon working"
# Then copy team_prompts/RECOVERY_JUNIOR_ENGINEER.md
# Paste as first message to Claude
```

**QA Validator**:
```bash
# Deploy immediately (parallel with senior)
# Copy team_prompts/RECOVERY_QA_VALIDATOR.md
# Paste as first message to Claude
```

## 🔄 Coordination Protocol

### Tech Lead Monitoring
```bash
# Monitor all teams
watch -n 30 'npx claude-flow@alpha swarm status'

# Check progress
npx claude-flow@alpha hooks query --all --status

# View team updates
tail -f /landropic/recovery/*-progress.log
```

### Inter-team Dependencies

**Senior → Systems**:
- Senior must report "compilation fixed" before systems begins
- Systems waits for clean build before module integration

**Systems → Junior**:
- Systems must report "daemon working" before junior begins CLI
- Junior needs working daemon for CLI testing

**QA → All**:
- QA validates all fixes in real-time
- QA blocks progress if regressions detected

## 📊 Progress Tracking

### Quality Gates

**Gate 1: Compilation (Hour 8)**
```bash
cargo build --workspace && echo "✅ GATE 1 PASSED" 
```

**Gate 2: Integration (Hour 16)** 
```bash
timeout 3 ./target/debug/landro-daemon --help && echo "✅ GATE 2 PASSED"
```

**Gate 3: User Ready (Hour 24)**
```bash
./target/debug/landro-cli --help && echo "✅ GATE 3 PASSED"
```

### Success Metrics
- **Hour 8**: All packages compile
- **Hour 16**: Daemon responds to --help  
- **Hour 24**: CLI basic commands work
- **Hour 48**: End-to-end functionality
- **Hour 72**: Release ready

## 🚨 Emergency Procedures

### If Senior Engineer Blocked (Hour 4+)
1. Tech lead reviews compilation errors directly
2. Consider scope reduction (disable complex modules)
3. Deploy additional senior engineer if needed

### If Systems Engineer Blocked (Hour 12+)  
1. Simplify integration (stub out complex features)
2. Focus on daemon stability over feature integration
3. Consider rolling back problematic modules

### If Timeline Slipping
1. **Hour 12**: Assess if on track for 72-hour target
2. **Hour 24**: Consider scope reduction decisions
3. **Hour 48**: Make go/no-go call for extended timeline

## 🔧 Tech Lead Decision Authority

**Immediate Decisions:**
- Resource reallocation between teams
- Scope reduction (disable features if needed)
- Quality vs speed tradeoffs
- Emergency architecture changes

**Escalation Triggers:**
- No progress after 25% of allocated time
- New critical issues discovered
- Team member completely blocked
- Quality gate failures persist

## 📈 Daily Checkpoints

### Morning Review (9 AM)
- Review overnight progress
- Adjust team assignments if needed
- Unblock any stuck engineers
- Update timeline estimates

### Midday Check (2 PM)
- Validate quality gates
- Coordinate handoffs between teams
- Address integration issues
- Plan afternoon priorities

### Evening Summary (6 PM)
- Progress against timeline
- Issues for overnight consideration
- Next day planning
- Stakeholder updates

## 💬 Communication Channels

### Team Updates
```bash
# Standard format for progress reports
echo "TEAM [senior|systems|junior|qa] STATUS [Hour X]:
- Current task: [specific work]
- Progress: [percentage or milestone]
- Blockers: [any impediments]
- ETA: [realistic timeline]
- Next: [upcoming work]
" | npx claude-flow@alpha hooks notify
```

### Escalation Process
1. **Try to unblock yourself**: 30 minutes max
2. **Check coordination memory**: Look for similar issues
3. **Notify team**: Use hooks to request help
4. **Tech lead intervention**: If blocked > 1 hour
5. **All-hands coordination**: If multiple teams blocked

## 📋 Recovery Success Criteria

### Minimum Recovery (72 hours)
- [ ] All code compiles without errors
- [ ] CLI binary works with basic commands  
- [ ] Daemon starts, stops, responds to --help
- [ ] Integration tests run (may not all pass)
- [ ] No hanging or crashing on normal usage

### Full Recovery (1 week)
- [ ] Basic file sync works between two nodes
- [ ] CLI provides complete user interface
- [ ] All tests pass
- [ ] Documentation updated
- [ ] Ready for alpha release

### Stretch Recovery (2 weeks)
- [ ] Advanced features from sprint working
- [ ] Performance optimizations active
- [ ] Security features operational
- [ ] Production deployment ready

## 🎯 Final Go/No-Go Decision

**Tech Lead will make final call based on:**
- All quality gates passed
- Basic user scenarios working
- No critical stability issues
- Team confidence in release
- Market timing considerations

**Possible Outcomes:**
- ✅ **Ship v0.0.1-alpha**: All criteria met
- 🔄 **Extended timeline**: Need more time for quality
- 📦 **Scope reduction**: Ship with fewer features  
- 🚧 **Dev preview**: Ship as development preview only

---

## 🚀 Deploy the Recovery Teams!

**Each team member gets a complete mission brief, clear success criteria, and direct coordination support.**

**Tech lead provides continuous oversight and unblocking support.**

**Quality is protected by continuous validation.**

**Timeline is realistic but focused on getting working software shipped.**

**Let's recover and ship landropic v0.0.1-alpha!** 🎯