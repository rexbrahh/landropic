# Cross-Platform Test Plan - Linux & Windows

**Junior Engineer:** landropic-junior-engineer-4  
**Date:** 2025-09-06  
**Status:** PLANNED (Awaiting compilation fixes)

## Linux Testing Environment Setup

### Target Distributions
1. **Ubuntu 22.04 LTS** (Primary)
   - Most common desktop Linux
   - systemd-based service management
   - APT package manager
   - Default: ext4 file system

2. **Fedora 40** (Secondary)
   - Cutting-edge kernel features
   - RPM package manager
   - Btrfs by default (CoW testing)
   - SELinux enabled

3. **Alpine Linux** (Container/Minimal)
   - musl libc instead of glibc
   - Minimal footprint
   - Good for edge case testing

### Linux-Specific Test Areas

#### File Systems
- **ext4**: Most common, case-sensitive, good performance
- **Btrfs**: Copy-on-write, snapshots, compression
- **XFS**: High-performance for large files
- **ZFS**: If available, advanced features

#### Permissions & Security
- **Standard Unix permissions** (owner/group/other)
- **Extended attributes** (security.*, user.*, system.*)
- **SELinux contexts** (Fedora, RHEL)
- **AppArmor profiles** (Ubuntu, openSUSE)
- **Capabilities** (CAP_NET_BIND_SERVICE for daemon)

#### Service Management
- **systemd** (most distributions)
  - Service unit files
  - Socket activation
  - User services vs system services
- **init.d** (legacy systems)
- **Supervisor/systemctl** alternatives

#### Network Configuration
- **NetworkManager** vs **systemd-networkd**
- **iptables/netfilter** firewall rules
- **mDNS via Avahi** (systemd-resolved)
- **Interface naming** (predictable network interface names)

### Linux Testing Strategy

#### Phase 1: Core Functionality
- [ ] Compile on x86_64 Linux
- [ ] Compile on ARM64 Linux (Raspberry Pi)
- [ ] Basic daemon startup/shutdown
- [ ] Service discovery (Avahi integration)
- [ ] File sync operations

#### Phase 2: File System Testing
- [ ] Case-sensitive filename handling
- [ ] Extended attributes preservation
- [ ] Large file support (>4GB)
- [ ] Sparse file handling
- [ ] Symbolic link vs hard link behavior

#### Phase 3: System Integration
- [ ] systemd service installation
- [ ] User vs system daemon modes
- [ ] Firewall integration
- [ ] Automatic startup/recovery

#### Phase 4: Distribution Variants
- [ ] Ubuntu/Debian (glibc, ext4)
- [ ] Fedora/RHEL (glibc, Btrfs/XFS)
- [ ] Alpine (musl libc, minimal)
- [ ] Container environments (Docker)

## Windows Testing Environment Setup

### Target Versions
1. **Windows 11** (Primary)
   - Latest APIs and security features
   - Modern file system features
   - Windows Subsystem for Linux (WSL) available

2. **Windows 10** (Compatibility)
   - Still widely deployed
   - Different security model nuances
   - Legacy compatibility requirements

### Windows-Specific Test Areas

#### File Systems
- **NTFS**: Primary file system
  - Case-insensitive by default
  - Alternate data streams (ADS)
  - NTFS compression
  - Long filename support
- **ReFS**: Resilient File System (if available)
  - Modern features
  - Better integrity checking

#### Path Handling
- **Backslash separators** (`\` vs `/`)
- **Drive letters** (`C:\` vs Unix root)
- **UNC paths** (`\\server\share`)
- **Long path support** (>260 characters, Win10+)
- **Reserved names** (CON, PRN, AUX, NUL, COM1-9, LPT1-9)

#### Permissions & Security
- **Windows ACLs** (Access Control Lists)
- **UAC** (User Account Control) integration
- **Windows Defender** exclusions needed
- **Windows Firewall** configuration
- **Execution policies** (for PowerShell scripts)

#### Service Management
- **Windows Service Control Manager** (SCM)
- **Service installation** via sc.exe or WiX
- **Service recovery** options
- **Event Log** integration
- **Service dependencies**

#### Network Configuration
- **Windows networking stack** differences
- **WinSock** vs Unix sockets
- **Bonjour for Windows** (if needed)
- **mDNS responder** installation

### Windows Testing Strategy

#### Phase 1: Basic Functionality
- [ ] Cross-compile from Linux/macOS
- [ ] Native Windows compilation
- [ ] Basic daemon/service operation
- [ ] File sync functionality

#### Phase 2: Windows-Specific Features
- [ ] Service installation/removal
- [ ] UAC integration
- [ ] Long path handling
- [ ] Reserved filename handling
- [ ] Alternate data streams

#### Phase 3: System Integration
- [ ] Windows Service operation
- [ ] Firewall configuration
- [ ] Event Log integration
- [ ] Automatic startup

#### Phase 4: Compatibility Testing
- [ ] Windows 10 vs Windows 11
- [ ] Different editions (Home/Pro/Enterprise)
- [ ] WSL integration (if applicable)
- [ ] Container support (Windows containers)

## Cross-Platform Test Cases

### File System Compatibility
1. **Unicode Normalization**
   - macOS: NFD (Normalization Form Decomposed)
   - Linux/Windows: NFC (Normalization Form Composed)
   - Test: Files with accented characters

2. **Case Sensitivity Matrix**
   - macOS: Case-insensitive (default)
   - Linux: Case-sensitive
   - Windows: Case-insensitive
   - Test: `FILE.txt` and `file.txt` in same directory

3. **Special Characters**
   - `:` (colon) - illegal on Windows
   - `<>|*?"` - reserved on Windows
   - `\0` (null) - illegal on all systems
   - Leading/trailing spaces and dots

4. **Path Length Limits**
   - macOS: 1024 bytes typically
   - Linux: 4096 bytes (PATH_MAX)
   - Windows: 260 chars (legacy), 32,767 with long path support

### Network Behavior
1. **mDNS Service Discovery**
   - macOS: Built-in Bonjour
   - Linux: Avahi daemon
   - Windows: Bonjour for Windows or native implementation

2. **Firewall Integration**
   - Port opening/closing
   - Service exceptions
   - Network interface binding

### Performance Characteristics
1. **File I/O Patterns**
   - Small files vs large files
   - Concurrent access patterns
   - Memory mapping support

2. **Network Stack Performance**
   - QUIC implementation performance
   - Connection establishment latency
   - Throughput comparison

## Testing Tools & Infrastructure

### Virtualization Options
- **Docker/Podman**: Linux containers
- **VirtualBox/VMware**: Full OS virtualization
- **QEMU**: Cross-architecture emulation
- **Windows Subsystem for Linux**: Windows/Linux hybrid

### CI/CD Integration
- **GitHub Actions**: Multi-platform runners
- **GitLab CI**: Docker-based testing
- **Azure DevOps**: Windows-specific testing

### Automated Testing Framework
```bash
# Example test structure
tests/
├── platform/
│   ├── macos/
│   ├── linux/
│   └── windows/
├── cross-platform/
│   ├── unicode/
│   ├── paths/
│   └── sync/
└── integration/
    ├── discovery/
    ├── transfer/
    └── recovery/
```

## Deployment Considerations

### Package Distribution
- **macOS**: `.dmg` installer, Homebrew
- **Linux**: `.deb`, `.rpm`, AppImage, Snap, Flatpak
- **Windows**: `.msi` installer, Chocolatey, winget

### Service Installation
- **macOS**: `launchd` plist files
- **Linux**: systemd unit files
- **Windows**: Windows Service SCM

## Risk Assessment

### High-Risk Areas
1. **File System Differences**: Unicode, case sensitivity
2. **Network Stack Variations**: mDNS, firewall integration
3. **Service Management**: Different init systems
4. **Security Models**: Permissions, sandboxing

### Mitigation Strategies
1. **Extensive Unit Testing**: Platform-specific test suites
2. **Integration Testing**: Cross-platform sync scenarios
3. **User Acceptance Testing**: Real-world usage patterns
4. **Automated CI/CD**: Continuous platform validation

## Current Blockers & Next Steps

### Immediate Blockers
- **CAS Compilation Errors**: Must be fixed first
- **Missing Test Infrastructure**: Need VM/container setup
- **Cross-compilation**: Rust toolchain setup

### Preparation Steps (Can Start Now)
- [ ] Set up Linux VM environments
- [ ] Prepare Windows testing environment
- [ ] Create cross-platform test frameworks
- [ ] Document platform-specific requirements
- [ ] Design CI/CD pipeline structure

---
**Memory Key:** `swarm/junior4/cross-platform-plan`  
**Status:** Ready for execution once compilation issues resolved  
**Dependencies:** CAS layer fixes, VM/container infrastructure