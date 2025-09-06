# Landropic CLI Reference

## Overview

The `landropic` command-line interface provides complete control over file synchronization, device pairing, and daemon management. All commands support both interactive and JSON output modes for integration with scripts and other tools.

## Global Options

```bash
landropic [OPTIONS] <COMMAND>
```

### Options
- `-v, --verbose` - Increase logging verbosity (can be used multiple times: -v, -vv, -vvv)
- `--help` - Display help information
- `--version` - Display version information

## Commands

### `config` - Configuration Management

Manage landropic configuration settings.

```bash
landropic config <SUBCOMMAND>
```

#### Subcommands

##### `show` - Show Current Configuration
```bash
landropic config show [OPTIONS]
```

**Options:**
- `--json` - Output in JSON format

##### `get` - Get Configuration Value
```bash
landropic config get <KEY>
```

**Arguments:**
- `<KEY>` - Configuration key (device_name, device_id, daemon_port, storage_path)

##### `set` - Set Configuration Value
```bash
landropic config set <KEY> <VALUE>
```

**Arguments:**
- `<KEY>` - Configuration key to set (device_name, daemon_port, storage_path)
- `<VALUE>` - Value to set

##### `reset` - Reset Configuration
```bash
landropic config reset [OPTIONS]
```

**Options:**
- `--force` - Confirm reset operation

#### Examples
```bash
# Show current configuration
landropic config show

# Get device name
landropic config get device_name

# Set device name
landropic config set device_name "My Laptop"

# Set daemon port
landropic config set daemon_port 8080

# Reset configuration (requires --force)
landropic config reset --force
```

### `init` - Initialize Landropic

Initialize landropic on this device, creating necessary configuration and identity keys.

```bash
landropic init [OPTIONS]
```

#### Options
- `-n, --name <NAME>` - Device name for identification (default: hostname)

#### Examples
```bash
# Initialize with default hostname
landropic init

# Initialize with custom device name
landropic init --name "Work Laptop"
```

### `pair` - Device Pairing

Pair this device with another landropic device for secure file synchronization.

```bash
landropic pair [OPTIONS]
```

#### Options
- `--show-qr` - Display QR code for pairing (conflicts with --code)
- `--code <CODE>` - Enter pairing code from another device (conflicts with --show-qr)

#### Examples
```bash
# Display QR code for another device to scan
landropic pair --show-qr

# Pair using code from another device
landropic pair --code "ABC123DEF456..."
```

### `sync` - Synchronize Folders

Start synchronizing a folder with paired devices.

```bash
landropic sync <FOLDER> [OPTIONS]
```

#### Arguments
- `<FOLDER>` - Path to folder to synchronize

#### Options
- `-w, --watch` - Enable continuous monitoring for changes

#### Examples
```bash
# One-time sync of Documents folder
landropic sync ~/Documents

# Continuous sync with file watching
landropic sync ~/Documents --watch

# Sync with verbose logging
landropic sync ~/Projects -vv --watch
```

### `status` - Check Sync Status

Display current synchronization status and progress.

```bash
landropic status [OPTIONS]
```

#### Options
- `--json` - Output in JSON format
- `-f, --folder <FOLDER>` - Show status for specific folder only

#### Examples
```bash
# Show overall sync status
landropic status

# Show status for specific folder
landropic status --folder ~/Documents

# Get status as JSON for scripting
landropic status --json
```

### `list` - List Synced Folders

Display all folders currently being synchronized.

```bash
landropic list [OPTIONS]
```

#### Options
- `--json` - Output in JSON format

#### Examples
```bash
# List all synced folders
landropic list

# Get list as JSON
landropic list --json
```

### `unsync` - Stop Syncing

Stop synchronizing a specific folder.

```bash
landropic unsync <FOLDER>
```

#### Arguments
- `<FOLDER>` - Path to folder to stop syncing

#### Examples
```bash
# Stop syncing Documents folder
landropic unsync ~/Documents
```

### `peers` - List Paired Devices

Display all devices paired with this system.

```bash
landropic peers [OPTIONS]
```

#### Options
- `--json` - Output in JSON format

#### Examples
```bash
# List all paired devices
landropic peers

# Get peer list as JSON
landropic peers --json
```

### `daemon` - Daemon Management

Manage the landropic background daemon service.

```bash
landropic daemon <SUBCOMMAND>
```

#### Subcommands

##### `start` - Start Daemon
```bash
landropic daemon start
```

##### `stop` - Stop Daemon
```bash
landropic daemon stop
```

##### `restart` - Restart Daemon
```bash
landropic daemon restart
```

##### `status` - Check Daemon Status
```bash
landropic daemon status
```

#### Examples
```bash
# Start the background daemon
landropic daemon start

# Check if daemon is running
landropic daemon status

# Restart daemon after configuration changes
landropic daemon restart

# Stop the daemon
landropic daemon stop
```

## Exit Codes

- `0` - Success
- `1` - General error
- `2` - Configuration error
- `3` - Connection error
- `4` - Authentication error
- `5` - File system error

## Environment Variables

- `LANDROPIC_CONFIG_DIR` - Override default configuration directory
- `LANDROPIC_DATA_DIR` - Override default data directory
- `RUST_LOG` - Control logging level (error, warn, info, debug, trace)

## Configuration Files

Configuration is stored in platform-specific locations:

### Linux
- Config: `~/.config/landropic/`
- Data: `~/.local/share/landropic/`

### macOS
- Config/Data: `~/Library/Application Support/landropic/`

### Windows
- Config/Data: `%APPDATA%\landropic\`

## Logging

Control logging verbosity using the `-v` flag or `RUST_LOG` environment variable:

```bash
# Basic logging
landropic -v sync ~/Documents

# Detailed logging
landropic -vv sync ~/Documents

# Debug logging
RUST_LOG=debug landropic sync ~/Documents

# Trace logging for troubleshooting
RUST_LOG=trace landropic daemon start
```

## Common Workflows

### Initial Setup
```bash
# 1. Initialize landropic
landropic init --name "My Computer"

# 2. Start the daemon
landropic daemon start

# 3. Pair with another device
landropic pair --show-qr
```

### Adding a Sync Folder
```bash
# 1. Start syncing a folder
landropic sync ~/Documents --watch

# 2. Check sync status
landropic status --folder ~/Documents

# 3. List all synced folders
landropic list
```

### Troubleshooting
```bash
# Check daemon status
landropic daemon status

# View detailed logs
RUST_LOG=debug landropic daemon restart

# Check peer connectivity
landropic peers

# Force resync
landropic unsync ~/Documents
landropic sync ~/Documents --watch
```

## See Also

- [User Guide](USER_GUIDE.md) - Detailed usage instructions
- [API Documentation](API_REFERENCE.md) - Library API reference
- [Architecture](../architechture.md) - System architecture overview