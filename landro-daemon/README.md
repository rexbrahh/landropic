# landro-daemon

The daemon module provides the core background services for landropic, including peer discovery, file watching, and sync coordination.

## Features

### mDNS Service Discovery

The discovery service provides automatic peer detection on local networks using mDNS/Bonjour:

- **Service Advertising**: Announces this device's presence with service type `_landropic._tcp`
- **Peer Discovery**: Automatically discovers other landropic instances
- **Network Events**: Real-time notifications of peer changes
- **Cross-platform**: Works on macOS (Bonjour), Linux (Avahi), and Windows
- **Resilient**: Automatic retry logic and graceful error handling

#### Basic Usage

```rust
use std::sync::Arc;
use landro_crypto::DeviceIdentity;
use landro_daemon::discovery::{Discovery, DiscoveryEvent};

// Create device identity and discovery service
let identity = Arc::new(DeviceIdentity::generate("My Device")?);
let mut discovery = Discovery::new(identity);

// Subscribe to events
let mut events = discovery.subscribe();

// Start advertising and browsing
discovery.start_advertising(12345, vec!["sync".to_string()]).await?;
discovery.start_browsing().await?;

// Handle events
while let Ok(event) = events.recv().await {
    match event {
        DiscoveryEvent::PeerDiscovered(peer) => {
            println!("Found peer: {} at {}:{}", peer.device_name, peer.address, peer.port);
        }
        DiscoveryEvent::PeerRemoved(device_id) => {
            println!("Lost peer: {}", device_id);
        }
        _ => {}
    }
}
```

#### Service Information

Each advertised service includes:

- **Device ID**: Ed25519 public key fingerprint for identification
- **Device Name**: Human-readable device name
- **Port**: QUIC service port number  
- **Capabilities**: Supported features (sync, backup, etc.)
- **Protocol Version**: Wire protocol version for compatibility

#### Network Resilience

The discovery service handles:

- **Interface Changes**: Automatically re-advertises when network interfaces change
- **Service Failures**: Retry logic with exponential backoff
- **Graceful Shutdown**: Proper cleanup of mDNS registrations
- **Error Recovery**: Automatic restart of failed components

#### Event System

The discovery service provides real-time events via broadcast channels:

- `PeerDiscovered(PeerInfo)`: New peer found on network
- `PeerRemoved(String)`: Peer no longer available  
- `PeerUpdated(PeerInfo)`: Peer information changed
- `ServiceRegistered`: Our service successfully registered
- `ServiceUnregistered`: Our service removed from network
- `NetworkError(String)`: Network-related error occurred

### Integration with Daemon

The discovery service integrates seamlessly with the main daemon lifecycle:

```rust
use landro_daemon::Daemon;

let daemon = Daemon::new();
daemon.start().await?; // This would initialize discovery internally

// The daemon coordinates:
// - Discovery service startup/shutdown
// - QUIC server port registration
// - Peer event handling
// - Connection management
```

### Running the Demo

To see the discovery service in action:

```bash
cargo run --example discovery_demo
```

This will:
1. Generate a device identity
2. Start advertising on port 12345
3. Begin scanning for peers
4. Show real-time discovery events
5. Gracefully shutdown on Ctrl+C

### Error Handling

The discovery service defines specific error types:

- `DaemonStart`: Failed to initialize mDNS daemon
- `ServiceRegistration`: Could not register service
- `ServiceBrowsing`: Failed to start peer discovery
- `InvalidServiceData`: Malformed peer information
- `AlreadyRegistered`: Service already advertised

### Security Considerations

- Device IDs are derived from cryptographic keys, providing strong identity
- TXT records contain only non-sensitive metadata
- Actual communication security is handled by the QUIC transport layer
- Service discovery is limited to the local network segment

### Performance

- Minimal CPU and memory overhead
- Efficient event-driven architecture
- Automatic resource cleanup
- Suitable for battery-powered devices

### Platform Support

| Platform | Backend | Status |
|----------|---------|--------|
| macOS    | Bonjour | ✅ Full support |
| Linux    | Avahi   | ✅ Full support |
| Windows  | Bonjour | ✅ Full support |

The `mdns-sd` crate handles platform-specific differences automatically.

## Testing

Run the discovery service tests:

```bash
cargo test -p landro-daemon discovery::tests
```

The test suite covers:

- Service registration and browsing
- Event subscription and handling
- Error conditions and recovery
- Peer information parsing
- Cross-device scenarios