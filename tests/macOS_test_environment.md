# macOS Test Environment - Detailed Specification

**Junior Engineer:** landropic-junior-engineer-4  
**Date:** 2025-09-06  
**Status:** READY (Pending compilation fixes)

## Hardware Environment

### System Information
- **Model:** MacBook Pro (M4 Pro)
- **Model Identifier:** Mac16,7
- **Chip:** Apple M4 Pro (ARM64 architecture)
- **System Version:** macOS 15.6.1 (24G90)
- **Kernel Version:** Darwin 24.6.0

### File System Configuration
- **Type:** APFS (Apple File System)
- **Attributes:** sealed, local, read-only root, journaled
- **Capacity:** 460GB total, 215GB available
- **Case Sensitivity:** Case-insensitive by default (configurable)
- **Device:** `/dev/disk3s3s1`

## Development Environment

### Rust Toolchain
- **Version:** 1.89.0 (29483883e 2025-08-04)
- **Toolchain:** stable-aarch64-apple-darwin
- **Cargo Version:** 1.89.0 (c24e10642 2025-06-23)
- **Target Triple:** aarch64-apple-darwin

### Build Configuration
- **RUSTC_WRAPPER:** Disabled (sccache causing issues)
- **CARGO_BUILD_JOBS:** Unset (was causing job=0 error)
- **Target Architecture:** ARM64 native compilation

## macOS-Specific Testing Focus Areas

### File System Behavior
1. **Case Sensitivity Edge Cases**
   - Default: Case-insensitive, case-preserving
   - Test: `FILE.txt` vs `file.txt` handling
   - Sync behavior when switching between case-sensitive/insensitive volumes

2. **Extended Attributes**
   - Resource forks (legacy but still possible)
   - Custom extended attributes (xattr)
   - Metadata preservation during sync

3. **Special Files & Directories**
   - `.DS_Store` files (Finder metadata)
   - `._` AppleDouble files (resource fork data on non-HFS volumes)
   - `.localized` directory localization
   - Symbolic links vs aliases

4. **Path Handling**
   - Unicode normalization (NFD on macOS vs NFC on other systems)
   - Maximum path length: 1024 bytes (typically)
   - Special characters in filenames

### APFS-Specific Features
1. **Copy-on-Write (CoW)**
   - File cloning behavior
   - Snapshot integration
   - Space efficiency testing

2. **Compression**
   - Automatic compression for suitable files
   - Impact on sync checksums
   - Performance implications

3. **Encryption**
   - FileVault integration
   - Per-file encryption keys
   - Secure deletion behavior

### Security & Permissions
1. **Gatekeeper Integration**
   - Code signing requirements
   - Quarantine attributes on downloaded files
   - Notarization implications

2. **System Integrity Protection (SIP)**
   - Protected directories
   - Restricted file operations
   - Impact on daemon installation

3. **Privacy Permissions**
   - Full Disk Access requirements
   - Documents folder access
   - Network access permissions

### Network Stack
1. **Bonjour/mDNS**
   - Service discovery testing
   - Multicast DNS resolution
   - Network interface enumeration

2. **Firewall Interaction**
   - macOS Application Layer Firewall
   - Network extension framework
   - Connection acceptance prompts

### Performance Characteristics
1. **File I/O Patterns**
   - Optimized for small files (common on macOS)
   - SSD-optimized operations
   - Memory pressure handling

2. **Background Processing**
   - Grand Central Dispatch integration
   - Energy efficiency requirements
   - App Nap considerations

## Testing Strategy (When Compilation Fixed)

### Phase 1: Basic Functionality
- [ ] Daemon startup and shutdown
- [ ] Service discovery via mDNS
- [ ] Basic file sync operations
- [ ] QUIC connection establishment

### Phase 2: File System Edge Cases
- [ ] Unicode filename handling
- [ ] Extended attribute preservation
- [ ] Large file transfers (>4GB)
- [ ] Special macOS file types

### Phase 3: System Integration
- [ ] Background daemon operation
- [ ] System permission handling
- [ ] Energy efficiency validation
- [ ] Memory usage profiling

### Phase 4: Multi-Device Testing
- [ ] Mac-to-Mac sync
- [ ] Cross-platform sync (pending other platforms)
- [ ] Network topology changes
- [ ] Connection recovery testing

## Known Limitations & Workarounds

### Current Blocking Issues
- **CAS Compilation Errors:** Preventing any testing
- **Missing Dependencies:** Some workspace deps were missing (fixed)

### Platform-Specific Considerations
- **Executable Permissions:** May need special handling for synced executables
- **Bundle Structures:** `.app` bundles should be treated atomically
- **Spotlight Integration:** Consider `.noindex` for sync directories
- **Time Machine:** Exclude patterns for backup software

## Testing Tools Available

### Built-in macOS Tools
- `fs_usage`: File system call tracing
- `dtrace`: System-wide tracing
- `lsof`: Open file listing
- `netstat`: Network connection monitoring
- `dscl`: Directory service testing

### Third-party Tools (if needed)
- `fswatch`: File system event monitoring
- `iperf3`: Network performance testing
- `wireshark`: Network protocol analysis

## Next Steps

1. **BLOCKED:** Wait for storage engineer to fix CAS compilation errors
2. **READY:** Execute Phase 1 testing once compilation succeeds
3. **PREPARE:** Set up automated testing scripts for reproducible results
4. **COORDINATE:** Share results with cross-platform testing team

---
**Memory Key:** `swarm/junior4/macos-environment`  
**Dependencies:** CAS layer compilation fixes from storage engineer