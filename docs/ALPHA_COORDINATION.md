# Alpha Release Coordination Plan - Final Push

## 🎯 Mission: Ship v0.0.1-alpha in 3-5 Days

**Current Status**: Critical blockers identified, focused fixes needed
**Strategy**: Deploy 4 specialized teams working concurrently

## 🚀 Team Deployment Instructions

### Deploy All Teams NOW (Concurrent Execution)

Open 4 separate Claude Code sessions and paste the respective prompts:

1. **Systems Engineer** (`team_prompts/ALPHA_SYSTEMS_ENGINEER.md`)
   - Fix QuicSyncTransport panic (todo!() at line 98)
   - Add daemon argument parsing
   - Priority: CRITICAL - Blocks everything

2. **Senior Engineer** (`team_prompts/ALPHA_SENIOR_ENGINEER.md`)
   - Implement basic file transfer protocol
   - Create simple one-way sync
   - Priority: HIGH - Core functionality

3. **Junior Engineer** (`team_prompts/ALPHA_JUNIOR_ENGINEER.md`)
   - Build working CLI binary
   - Implement daemon control commands
   - Priority: HIGH - User interface

4. **QA Validator** (`team_prompts/ALPHA_QA_VALIDATOR.md`)
   - Continuous testing and validation
   - Prevent regressions
   - Priority: CONTINUOUS - Quality assurance

## 📋 Critical Path & Dependencies

```
Day 1 (First 12 hours):
├── Systems Engineer: Fix panic, add argument parsing
├── Junior Engineer: Build CLI structure
└── QA: Monitor builds continuously

Day 2 (Next 12 hours):
├── Systems Engineer: Complete transport methods
├── Senior Engineer: Implement sync protocol
├── Junior Engineer: Complete daemon control
└── QA: Test daemon stability

Day 3 (Final 12 hours):
├── Senior Engineer: Wire up file transfer
├── Junior Engineer: Add sync command
├── All: Integration testing
└── QA: Full validation suite

Day 4-5 (Polish & Release):
├── Fix any remaining issues
├── Documentation updates
├── Build release binaries
└── Tag v0.0.1-alpha
```

## 🔧 Technical Fixes Required

### Critical Blocker #1: QuicSyncTransport Panic
```rust
// Current (BROKEN):
todo!("QuicSyncTransport needs actual connection from daemon client")

// Fix needed:
impl QuicSyncTransport {
    pub fn new(client: Arc<QuicClient>, pool: Arc<ConnectionPool>) -> Self {
        Self { client, pool, active_connections: Default::default() }
    }
}
```

### Critical Blocker #2: No CLI Binary
```bash
# Current: landro-cli doesn't exist
# Fix: Create working CLI with basic commands
cargo build --release -p landro-cli
```

### Critical Blocker #3: No File Transfer
```rust
// Need minimal sync protocol:
enum SimpleSyncMessage {
    RequestFile(String),
    FileData { path: String, data: Vec<u8> },
    Acknowledge,
}
```

## 🎯 Success Metrics

### Hour 12 Checkpoint:
- [ ] Daemon starts without panic
- [ ] CLI binary builds
- [ ] --help and --version work

### Hour 24 Checkpoint:
- [ ] Daemon runs stable for 60+ seconds
- [ ] CLI can start/stop daemon
- [ ] Basic sync protocol exists

### Hour 36 Checkpoint:
- [ ] File transfer works (at least once)
- [ ] No data corruption
- [ ] All components integrated

### Hour 48-72 (Release Ready):
- [ ] All alpha criteria met
- [ ] QA validation passed
- [ ] Binaries ready for distribution

## 💬 Coordination Protocol

### Each Team Reports Progress:
```bash
# Every 2 hours, update status
echo "[TEAM] Hour X: [current status]" >> recovery/progress.log
```

### Blocking Issues:
```bash
# Immediate escalation if blocked
echo "BLOCKED: [team] needs [specific help]" >> recovery/blockers.log
```

### Tech Lead Monitors:
```bash
# Check every 2 hours
watch -n 7200 'tail -20 recovery/*.log'
```

## 📦 Release Criteria

### Minimum for v0.0.1-alpha:
1. ✅ Daemon starts and runs without crashing
2. ✅ CLI can control daemon (start/stop/status)
3. ✅ One file can transfer between two nodes
4. ✅ --help and --version work on both binaries
5. ✅ No data corruption during transfer

### Nice to Have (Can wait for v0.0.2):
- Progress reporting
- Multiple file sync
- Bidirectional sync
- Configuration files
- Encryption

## 🚨 Emergency Procedures

### If Still Blocked After 24 Hours:
1. **Scope Reduction**: Remove advanced features, focus on basics
2. **Stub Complex Parts**: Use mock implementations where needed
3. **Hardcode Values**: OK for alpha (ports, addresses, etc.)

### If Critical Path Slips:
- Systems Engineer blocker → All hands help with transport
- Senior Engineer blocker → Simplify protocol further
- Junior Engineer blocker → Manual testing acceptable

## 🏁 Final Release Process

Once all teams report success:

```bash
# 1. Run final validation
./qa/alpha_validation.sh

# 2. Build release binaries
cargo build --release --workspace

# 3. Create distribution
mkdir -p dist/landropic-v0.0.1-alpha
cp target/release/landro-daemon dist/landropic-v0.0.1-alpha/
cp target/release/landro dist/landropic-v0.0.1-alpha/
cp README.md dist/landropic-v0.0.1-alpha/

# 4. Test distribution
cd dist/landropic-v0.0.1-alpha
./landro --version  # Should show 0.0.1-alpha
./landro-daemon --help  # Should show help

# 5. Tag release
git tag -a v0.0.1-alpha -m "Alpha release: Basic file sync working"
git push origin v0.0.1-alpha
```

## 🎯 Let's Ship This Alpha!

**Remember:**
- Simple but WORKING is the goal
- Fix the panics first
- Basic functionality only
- We can enhance in future versions

**The mission is clear: Make it work, ship it, iterate!**

---

**Deploy the teams NOW and let's get v0.0.1-alpha shipped!** 🚀