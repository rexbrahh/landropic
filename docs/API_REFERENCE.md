# Landropic API Reference

## Overview

This document provides comprehensive API documentation for Landropic's Rust crates and public interfaces. Each crate exposes specific functionality for building secure, high-performance file synchronization applications.

## Crate Organization

Landropic is organized into focused crates with clear separation of concerns:

- **`landro-crypto`** - Cryptographic primitives and device identity
- **`landro-quic`** - QUIC transport layer and networking
- **`landro-cas`** - Content-addressable storage
- **`landro-daemon`** - Background service and orchestration
- **`landro-cli`** - Command-line interface
- **`landro-proto`** - Protocol buffer definitions
- **`landro-index`** - File indexing and watching
- **`landro-chunker`** - Content-defined chunking
- **`landro-sync`** - High-level synchronization engine

---

## `landro-crypto` - Cryptographic Primitives

Provides device identity management and cryptographic operations.

### Core Types

#### `DeviceIdentity`
Represents a device's Ed25519 identity for secure authentication.

```rust
use landro_crypto::{DeviceIdentity, DeviceId};

// Generate a new identity
let identity = DeviceIdentity::generate("my-device")?;

// Save to disk
identity.save(None).await?;

// Load existing identity
let identity = DeviceIdentity::load_or_generate("my-device").await?;

// Get device ID
let device_id: DeviceId = identity.device_id();
```

**Methods:**
- `generate(name: &str) -> Result<DeviceIdentity>`
- `load_or_generate(name: &str) -> Result<DeviceIdentity>`  
- `save(&self, path: Option<PathBuf>) -> Result<()>`
- `device_id(&self) -> DeviceId`
- `public_key(&self) -> &Ed25519PublicKey`
- `sign(&self, data: &[u8]) -> Ed25519Signature`

#### `DeviceId`
Blake3-based unique identifier for devices.

```rust
let device_id = DeviceId::from_public_key(&public_key);
println!("Device: {}", device_id); // Displays as hex string
```

#### `CertificateGenerator`
Generates X.509 certificates for TLS authentication.

```rust
use landro_crypto::{CertificateGenerator, CertificateVerifier};

// Generate TLS certificate from device identity
let (cert_chain, private_key) = 
    CertificateGenerator::generate_device_certificate(&identity)?;

// Create verifier for mutual TLS
let verifier = CertificateVerifier::new(trusted_device_ids)?;
```

### Error Handling

```rust
use landro_crypto::{CryptoError, Result};

match operation() {
    Err(CryptoError::InvalidKey) => { /* Handle invalid key */ },
    Err(CryptoError::FileSystem(e)) => { /* Handle file error */ },
    Err(CryptoError::Crypto(e)) => { /* Handle crypto error */ },
    Ok(result) => { /* Success */ }
}
```

---

## `landro-quic` - QUIC Transport

High-performance, secure networking with QUIC protocol support.

### Core Types

#### `QuicServer`
Accepts inbound QUIC connections from peers.

```rust
use landro_quic::{QuicServer, QuicConfig};
use std::sync::Arc;

let config = QuicConfig::default()
    .with_port(7703)
    .with_max_streams(1000);

let mut server = QuicServer::new(
    identity.clone(),
    certificate_verifier,
    config
);

server.start().await?;

// Accept connections
while let Some(connection) = server.accept().await? {
    tokio::spawn(handle_connection(connection));
}
```

#### `QuicClient`
Initiates outbound connections to peers.

```rust
use landro_quic::{QuicClient, Connection};

let client = QuicClient::new(identity, verifier, config).await?;
let connection: Connection = client.connect("192.168.1.2:7703").await?;

// Open bidirectional stream
let (mut send, mut recv) = connection.open_bi().await?;
send.write_all(b"Hello").await?;
```

#### `Connection`
Represents an established QUIC connection with stream management.

```rust
impl Connection {
    // Stream management
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream)>;
    pub async fn open_uni(&self) -> Result<SendStream>;
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream)>;
    pub async fn accept_uni(&self) -> Result<RecvStream>;
    
    // Connection info
    pub fn remote_address(&self) -> SocketAddr;
    pub fn device_id(&self) -> &DeviceId;
    pub fn stats(&self) -> ConnectionStats;
}
```

#### `ParallelTransferManager`
High-performance parallel file transfers.

```rust
use landro_quic::{ParallelTransferManager, ChunkProvider};

let manager = ParallelTransferManager::new(config);

// Send file with parallel chunks
let provider = ChunkProvider::from_file(file_path)?;
let result = manager.send_parallel(connection, provider).await?;

println!("Transferred {} bytes", result.bytes_transferred);
```

### Configuration

```rust
use landro_quic::QuicConfig;

let config = QuicConfig::default()
    .with_port(7703)
    .with_max_streams(1000)
    .with_max_concurrent_transfers(10)
    .with_keep_alive_interval(Duration::from_secs(30))
    .with_idle_timeout(Duration::from_secs(300))
    .with_congestion_control("bbr")
    .enable_0rtt(true);
```

---

## `landro-cas` - Content-Addressable Storage

Efficient storage and deduplication using content-addressed chunks.

### Core Types

#### `ContentStore`
Main interface for content-addressable storage operations.

```rust
use landro_cas::{ContentStore, ContentStoreConfig, ObjectRef};

let config = ContentStoreConfig::default()
    .with_compression(CompressionType::Zstd)
    .with_cache_size(1_000_000)
    .enable_verification(true);

let store = ContentStore::new(storage_path, config).await?;

// Store content
let content = b"Hello, world!";
let object_ref: ObjectRef = store.put(content).await?;

// Retrieve content
let retrieved: Vec<u8> = store.get(&object_ref).await?;
assert_eq!(content, &retrieved[..]);

// Check existence
if store.contains(&object_ref).await? {
    println!("Object exists");
}
```

#### `ObjectRef`
Content-addressed reference using Blake3 hash.

```rust
use landro_cas::ObjectRef;

// From hash bytes
let object_ref = ObjectRef::from_hash(hash_bytes);

// Get hash
let hash: &[u8; 32] = object_ref.hash();

// String representation
println!("Object: {}", object_ref); // Hex representation
```

#### `PackfileManager`
Manages efficient storage of objects in packfiles.

```rust
use landro_cas::PackfileManager;

let manager = PackfileManager::new(config);

// Store multiple objects efficiently  
let refs = manager.store_batch(&objects).await?;

// Compact storage
manager.compact().await?;

// Get statistics
let stats = manager.stats();
println!("Objects: {}, Size: {} MB", stats.object_count, stats.total_size_mb);
```

### Metrics and Monitoring

```rust
use landro_cas::{MetricsCollector, StorageMetrics};

let collector = MetricsCollector::new();
let metrics: StorageMetrics = collector.collect();

println!("Cache hit rate: {:.2}%", metrics.cache.hit_rate * 100.0);
println!("Deduplication ratio: {:.2}", metrics.dedup.savings_ratio);
```

---

## `landro-daemon` - Background Service

Orchestrates synchronization and provides the daemon API.

### Core Types

#### `Orchestrator`
Main coordination service managing sync operations.

```rust
use landro_daemon::{Orchestrator, DaemonConfig};

let config = DaemonConfig::load()?;
let orchestrator = Orchestrator::new(config).await?;

// Start background services
orchestrator.start().await?;

// Add sync folder
orchestrator.add_sync_folder(path, watch_mode).await?;

// Get sync status
let status = orchestrator.get_sync_status().await?;
```

#### `SyncEngine`
Handles file synchronization logic and coordination.

```rust  
use landro_daemon::SyncEngine;

let engine = SyncEngine::new(storage, network_client);

// Synchronize folder
let result = engine.sync_folder(folder_path).await?;

// Get progress
let progress = engine.get_progress(folder_path).await?;
```

---

## Common Patterns

### Error Handling

All crates follow consistent error handling patterns:

```rust
use landro_crypto::Result; // or landro_quic::Result, etc.

fn example() -> Result<()> {
    let result = risky_operation()?;
    Ok(result)
}

// Pattern matching on specific errors
match operation() {
    Ok(success) => handle_success(success),
    Err(error) => match error {
        SpecificError::NetworkTimeout => retry_operation(),
        SpecificError::AuthFailure => reauthenticate(),
        _ => handle_generic_error(error),
    }
}
```

### Async/Await Usage

All I/O operations are async and use Tokio:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let client = QuicClient::new(identity, verifier, config).await?;
    let connection = client.connect(address).await?;
    
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(data).await?;
    
    Ok(())
}
```

### Configuration Builders

Most components use builder patterns for configuration:

```rust
let config = QuicConfig::default()
    .with_port(7703)
    .with_max_streams(1000)
    .enable_0rtt(true)
    .with_idle_timeout(Duration::from_secs(300));

let store_config = ContentStoreConfig::default()
    .with_compression(CompressionType::Zstd)
    .with_cache_size(1_000_000)
    .enable_verification(true);
```

### Resource Management

Components implement proper resource cleanup:

```rust
// Resources are automatically cleaned up via Drop
{
    let server = QuicServer::new(identity, verifier, config);
    server.start().await?;
    // server automatically stops when dropped
}

// Explicit cleanup when needed
server.shutdown().await?;
```

## Integration Example

Here's a complete example integrating multiple crates:

```rust
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_quic::{QuicServer, QuicClient, QuicConfig};
use landro_cas::{ContentStore, ContentStoreConfig};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup device identity
    let identity = Arc::new(DeviceIdentity::load_or_generate("my-device").await?);
    
    // 2. Configure certificate verification
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    
    // 3. Setup content store
    let store_config = ContentStoreConfig::default()
        .with_compression(CompressionType::Zstd);
    let store = ContentStore::new("/tmp/landropic", store_config).await?;
    
    // 4. Configure QUIC transport
    let quic_config = QuicConfig::default()
        .with_port(7703)
        .with_max_streams(1000);
    
    // 5. Start server
    let mut server = QuicServer::new(
        identity.clone(),
        verifier.clone(),
        quic_config.clone()
    );
    server.start().await?;
    
    // 6. Setup client for connecting to peers
    let client = QuicClient::new(identity, verifier, quic_config).await?;
    
    println!("Landropic node started");
    Ok(())
}
```

## Testing Support

All crates provide testing utilities:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_identity_generation() {
        let identity = DeviceIdentity::generate("test-device").unwrap();
        assert!(!identity.device_id().to_string().is_empty());
    }
    
    #[tokio::test]
    async fn test_quic_connection() {
        let (server, client) = setup_test_pair().await;
        let connection = client.connect(server.address()).await.unwrap();
        assert!(connection.is_connected());
    }
}
```

## See Also

- [CLI Reference](CLI_REFERENCE.md) - Command-line interface documentation
- [Architecture](../architechture.md) - System architecture overview  
- [Security Model](../security_model.md) - Security design and threat model