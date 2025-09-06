# Landropic User Guide

## What is Landropic?

Landropic is a secure, cross-platform file synchronization tool designed for local networks. Think of it as "AirDrop for everyone" - it automatically discovers devices on your network and enables fast, encrypted file sharing without relying on cloud services.

### Key Benefits

- **🔒 Privacy-First**: All files are encrypted end-to-end with no cloud storage required
- **🚀 Fast Transfer**: Optimized for local network speeds with resumable transfers
- **🔍 Auto-Discovery**: Devices automatically find each other on the network
- **📁 Real-Time Sync**: Changes sync immediately across all paired devices
- **🖥️ Cross-Platform**: Works seamlessly on Linux, macOS, and Windows
- **⚡ Resume Support**: Interrupted transfers pick up where they left off

---

## Getting Started

### Installation

#### From Releases (Recommended)
1. Download the latest release for your platform from [GitHub Releases](https://github.com/landropic/landropic/releases)
2. Extract the archive and move `landropic` to your PATH:

**Linux/macOS:**
```bash
sudo mv landropic /usr/local/bin/
```

**Windows:**
- Move `landropic.exe` to a folder in your PATH
- Or use the Windows installer

#### Building from Source
```bash
git clone https://github.com/landropic/landropic.git
cd landropic
cargo build --release
sudo cp target/release/landropic /usr/local/bin/
```

### First-Time Setup

#### 1. Initialize Landropic
```bash
landropic init --name "My Laptop"
```

This creates:
- A unique device identity
- Configuration files
- Starts the background daemon

#### 2. Verify Installation
```bash
landropic daemon status
```

You should see: `✓ Daemon is running`

---

## Pairing Devices

Before you can sync files, devices need to be paired securely.

### Method 1: QR Code Pairing (Easiest)

**On the first device:**
```bash
landropic pair --show-qr
```

This displays a QR code containing pairing information.

**On the second device:**
Use any QR code scanner app to read the code, then enter it:
```bash
landropic pair --code "ABC123DEF456..."
```

### Method 2: Manual Code Entry

**On the first device:**
```bash
landropic pair --show-qr
```

Copy the code displayed below the QR code.

**On the second device:**
```bash
landropic pair --code "[paste code here]"
```

### Verify Pairing

Check that devices are connected:
```bash
landropic peers
```

You should see your paired devices listed with green dots (●) indicating they're connected.

---

## Synchronizing Files

### Basic Sync Operations

#### Sync a Folder Once
```bash
landropic sync ~/Documents/ProjectFiles
```

This performs a one-time sync of the folder across all paired devices.

#### Continuous Sync (Watch Mode)
```bash
landropic sync ~/Documents/ProjectFiles --watch
```

This continuously monitors the folder for changes and syncs them in real-time.

#### Check Sync Status
```bash
landropic status
```

Shows overview of all sync operations, connected peers, and transfer progress.

#### List Synced Folders
```bash
landropic list
```

### Real-World Examples

#### Example 1: Sharing a Project Folder
```bash
# On your work laptop
landropic sync ~/Projects/WebsiteRedesign --watch

# The folder automatically appears on your home desktop
# Any changes sync immediately
```

#### Example 2: Photo Sharing
```bash
# On your phone (if mobile version available) or tablet
landropic sync ~/Photos/VacationTrip

# Photos appear on all paired devices
# Large files resume if interrupted
```

#### Example 3: Document Collaboration
```bash
# Team member 1
landropic sync ~/SharedDocs --watch

# Team member 2  
landropic sync ~/SharedDocs --watch

# All edits sync in real-time across team
```

---

## Managing Sync Folders

### Adding Multiple Folders
```bash
# Sync different folders
landropic sync ~/Documents --watch
landropic sync ~/Projects/Current --watch
landropic sync ~/Photos --watch
```

### Stopping Sync
```bash
# Stop syncing a specific folder
landropic unsync ~/Documents

# Or stop all syncing
landropic daemon stop
```

### Monitoring Progress
```bash
# Overall status
landropic status

# Detailed status with JSON output
landropic status --json

# Status for specific folder
landropic status --folder ~/Documents
```

---

## Advanced Features

### Configuration Management

#### View Current Settings
```bash
landropic config show
```

#### Modify Settings
```bash
# Change device name
landropic config set device_name "John's MacBook Pro"

# Change daemon port (requires restart)
landropic config set daemon_port 8080

# Change storage location
landropic config set storage_path ~/landropic-data
```

#### Reset Configuration
```bash
landropic config reset --force
```

### Daemon Management

```bash
# Start the daemon
landropic daemon start

# Stop the daemon
landropic daemon stop

# Restart the daemon
landropic daemon restart

# Check daemon status
landropic daemon status
```

### Troubleshooting Mode

Enable verbose logging for troubleshooting:
```bash
# Verbose output
landropic -v sync ~/Documents

# Very verbose output
landropic -vv sync ~/Documents

# Debug level logging
RUST_LOG=debug landropic daemon start
```

---

## Understanding Landropic's Behavior

### How Sync Works

1. **File Watching**: Landropic monitors folders for changes using efficient filesystem events
2. **Chunking**: Large files are split into chunks for efficient transfer and deduplication
3. **Deduplication**: Identical chunks are stored only once, saving space and bandwidth
4. **Encryption**: All data is encrypted before leaving your device
5. **Network Transfer**: Files transfer directly between devices on your network
6. **Conflict Resolution**: When conflicts occur, both versions are preserved with timestamps

### What Gets Synced

**Included:**
- All files and folders in synced directories
- File permissions (on Unix-like systems)
- File timestamps
- Symbolic links (as links, not targets)

**Excluded by Default:**
- Hidden files starting with `.` (configurable)
- Temporary files (`.tmp`, `.swp`, etc.)
- System files (`Thumbs.db`, `.DS_Store`, etc.)
- Very large files (>2GB by default, configurable)

### Network Discovery

Landropic uses mDNS (Bonjour/Zeroconf) to find devices automatically:
- Devices advertise their presence when the daemon starts
- Discovery works within the same network subnet
- Devices connect using QUIC protocol for security and performance

---

## Security and Privacy

### Encryption

- **End-to-End**: Files are encrypted with ChaCha20-Poly1305 before transmission
- **Device Authentication**: Ed25519 public keys uniquely identify devices  
- **Transport Security**: QUIC with TLS 1.3 secures all network communication
- **Forward Secrecy**: Ephemeral keys ensure past communications remain secure

### What Landropic Doesn't Do

- **No Cloud Storage**: Files never leave your local network
- **No Central Server**: Direct device-to-device communication
- **No User Accounts**: Authentication is based on device identity
- **No Data Collection**: No telemetry or usage data sent anywhere

### Best Practices

1. **Secure Pairing**: Always verify device identity during pairing
2. **Network Security**: Use WPA3/WPA2 protected WiFi networks
3. **Regular Updates**: Keep Landropic updated for security fixes
4. **Device Management**: Remove old/unused devices from pairing list

---

## Common Use Cases

### Home Office Setup

**Scenario**: Sync work files between desktop and laptop

```bash
# On desktop
landropic init --name "Desktop Computer"
landropic pair --show-qr
landropic sync ~/WorkProjects --watch

# On laptop  
landropic init --name "Work Laptop"
landropic pair --code [from QR scan]
# Folder appears automatically, start watching
landropic sync ~/WorkProjects --watch
```

### Family Photo Sharing

**Scenario**: Share vacation photos across family devices

```bash
# On primary device
landropic sync ~/Photos/2024-Summer-Vacation

# Photos automatically sync to all family devices
# No need for cloud storage or email attachments
```

### Small Team Collaboration

**Scenario**: Real-time document collaboration

```bash
# Each team member runs:
landropic sync ~/TeamDocs --watch

# All changes sync immediately
# Version conflicts are handled automatically
```

### Development Environment Sync

**Scenario**: Keep development files in sync across machines

```bash
# Sync active project
landropic sync ~/code/current-project --watch

# Sync configuration files
landropic sync ~/.config/nvim --watch
```

---

## Troubleshooting

### Common Issues

#### "Daemon is not running" 
```bash
# Start the daemon manually
landropic daemon start

# Check if it started successfully
landropic daemon status
```

#### "Connection refused" / Cannot connect to peers
```bash
# Check network connectivity
ping [peer-ip-address]

# Verify daemon is running on both devices
landropic daemon status

# Check firewall settings (port 7703 by default)
# Linux: sudo ufw allow 7703
# macOS: Allow in System Preferences > Security & Privacy > Firewall
```

#### "Permission denied" errors
```bash
# Check file permissions
ls -la [problematic-folder]

# Ensure landropic has read/write access
sudo chown -R $USER [problematic-folder]
```

#### Sync is slow or stuck
```bash
# Check sync status
landropic status

# Restart daemon to clear any issues
landropic daemon restart

# Enable debug logging
RUST_LOG=debug landropic daemon start
```

#### Files not syncing
```bash
# Verify folder is being watched
landropic list

# Check for file size limits or exclusions
landropic status --folder [folder-path]

# Manually trigger sync
landropic unsync [folder-path]
landropic sync [folder-path] --watch
```

### Getting Help

#### Log Files

Logs are stored in platform-specific locations:
- **Linux**: `~/.local/share/landropic/logs/`
- **macOS**: `~/Library/Logs/landropic/`
- **Windows**: `%APPDATA%\landropic\logs\`

#### Enable Debug Logging
```bash
# Temporary debug mode
RUST_LOG=debug landropic daemon start

# Or with specific components
RUST_LOG=landro_quic=debug,landro_sync=info landropic daemon start
```

#### Collect Diagnostic Information
```bash
# System information
landropic daemon status
landropic peers
landropic status --json

# Network connectivity
ping [peer-ip]
telnet [peer-ip] 7703

# Configuration
landropic config show
```

#### Support Channels
- **GitHub Issues**: Report bugs and feature requests
- **GitHub Discussions**: Community help and questions  
- **Documentation**: Check API reference and architecture docs

---

## Tips and Best Practices

### Performance Optimization

1. **Network**: Use wired connections for large transfers when possible
2. **Storage**: Place Landropic data on fast storage (SSD preferred)  
3. **Folders**: Sync smaller, focused folders rather than entire home directories
4. **Exclusions**: Configure exclusions for unnecessary files (logs, caches, etc.)

### Organizing Sync Folders

```bash
# Good: Focused, project-specific folders
landropic sync ~/Projects/WebApp --watch
landropic sync ~/Documents/CurrentWork --watch

# Avoid: Very large, general folders
# landropic sync ~ --watch  # This would sync everything!
```

### Backup and Recovery

- **Config Backup**: Save `~/.config/landropic/` for device identity
- **Data Recovery**: Content is stored in `~/.local/share/landropic/objects/`
- **Device Re-pairing**: If identity is lost, re-pair with all devices

### Network Considerations

- **Bandwidth**: Landropic can use significant bandwidth during initial sync
- **Battery**: Continuous syncing may impact laptop battery life
- **Firewall**: Ensure port 7703 (or configured port) is accessible
- **NAT**: Complex network setups may require port forwarding

---

## What's Next?

### Exploring Advanced Features

Once comfortable with basic sync:
- Explore API documentation for custom integrations
- Set up automated workflows using JSON output
- Configure custom exclusion patterns
- Optimize performance for your specific use case

### Staying Updated

- **Releases**: Watch the GitHub repository for updates
- **Security**: Subscribe to security notifications
- **Features**: Join discussions about upcoming features
- **Community**: Share your use cases and help others

### Contributing

- **Bug Reports**: Help improve Landropic by reporting issues
- **Feature Requests**: Suggest improvements  
- **Documentation**: Help improve docs for other users
- **Code**: Contribute features and bug fixes (see [Developer Setup](DEVELOPER_SETUP.md))

---

## Appendix

### File Locations

#### Configuration
- **Linux**: `~/.config/landropic/config.json`
- **macOS**: `~/Library/Application Support/landropic/config.json`
- **Windows**: `%APPDATA%\landropic\config.json`

#### Data Storage
- **Linux**: `~/.local/share/landropic/`
- **macOS**: `~/Library/Application Support/landropic/`
- **Windows**: `%APPDATA%\landropic\`

#### Device Identity
- **All Platforms**: `[config-dir]/identity/device.key`

### Default Settings

```json
{
  "daemon_port": 7703,
  "discovery_port": 5353,
  "max_concurrent_transfers": 10,
  "chunk_size": "1MB",
  "compression": "zstd",
  "encryption": "chacha20poly1305",
  "file_watcher_debounce": "100ms"
}
```

### Port Usage

- **7703**: Main QUIC communication (configurable)
- **5353**: mDNS discovery (standard)
- **Ephemeral**: Additional ports for multiple streams

---

**Happy syncing! 🚀**