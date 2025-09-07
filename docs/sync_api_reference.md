# Landropic Sync API Reference

## Overview

The landropic sync engine provides one-way file synchronization over QUIC transport with chunked deduplication and progress tracking.

## Core Components

### 1. QuicSyncTransport

**Purpose**: Bridges sync protocol with QUIC streaming infrastructure

```rust
use landro_sync::QuicSyncTransport;
use landro_quic::connection::Connection;
use landro_cas::ContentStore;

// Create transport
let transport = QuicSyncTransport::new(connection, cas).await?;

// Start sync session
let session_id = transport.start_sync("/sync/folder", true).await?;

// Monitor stats
let stats = transport.get_stats(&session_id).await;
```

### 2. SyncEngine Integration

**New Message Types**:

```rust
pub enum SyncMessage {
    // ... existing messages
    PeerConnected { peer_id: String, connection: Arc<Connection> },
    // ... 
}
```

**Usage**:

```rust
// Send connection to sync engine
sync_tx.send(SyncMessage::PeerConnected {
    peer_id: "device_123".to_string(),
    connection: Arc::new(quic_connection),
}).await?;

// Sync request will use QUIC transport
sync_tx.send(SyncMessage::SyncRequest {
    peer_id: "device_123".to_string(), 
    path: PathBuf::from("/sync/folder"),
}).await?;
```

## Protocol Messages

### Core Protocol Flow

1. **Handshake**: `Hello` ↔ `Hello`
2. **Folder Negotiation**: `FolderSummary` ↔ `FolderSummary` 
3. **Manifest Exchange**: `Manifest` → `Want`
4. **Chunk Transfer**: `ChunkData` → `Ack` (repeated)
5. **Completion**: Final `Ack`

### Message Format

All messages are Protocol Buffer encoded over QUIC streams:

```
[MessageType:u8][Length:u32][ProtoMessage:bytes]
```

### Stream Types

- **Stream 0**: Control messages (Hello, FolderSummary, Want, Ack)
- **Streams 1-N**: Data transfer (ChunkData)

## Configuration

### StreamTransferConfig

```rust
// File sync optimized (default)
let config = StreamTransferConfig::file_sync_optimized();
// - 64 concurrent streams
// - 8MB stream buffers  
// - 64MB flow control window
// - Adaptive flow control enabled

// Diff sync optimized
let config = StreamTransferConfig::diff_sync_optimized(); 
// - 32 concurrent streams
// - 4MB stream buffers
// - Compression enabled
```

### SyncEngineConfig

```rust
let config = SyncEngineConfig {
    auto_sync: true,                    // Auto-sync on peer connection
    conflict_strategy: ConflictStrategy::NewerWins,
};
```

## API Reference

### QuicSyncTransport Methods

#### `new(connection, cas) -> Result<Self>`
Creates transport from QUIC connection and content store.

#### `start_sync(folder_path, is_initiator) -> Result<String>`
Starts sync session, returns session ID.

#### `get_stats(session_id) -> Option<TransferStats>`
Gets transfer statistics for session.

### TransferStats Structure

```rust
pub struct TransferStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub chunks_sent: usize,
    pub chunks_received: usize,
    pub files_added: usize,
    pub files_modified: usize, 
    pub files_deleted: usize,
}
```

## Integration Points

### With QUIC Network Layer

**Required**: 
- `Connection` object with bidirectional streams
- Device identity exchange via connection
- mTLS certificate validation

**Interface**:
```rust
// Network engineer provides:
let connection: Arc<Connection> = // QUIC connection
connection.client_handshake(device_id, device_name).await?;

// Sync engineer uses:
let transport = QuicSyncTransport::new(connection, cas).await?;
```

### With Storage Layer

**Required**:
- `ContentStore` for chunk read/write operations
- Blake3 hash verification
- Atomic writes

**Interface**:
```rust
// Storage engineer provides:
let cas: Arc<ContentStore> = // Content store

// Sync engineer uses:
let data = cas.read(&content_hash).await?;
cas.write(&data).await?;
```

### With CLI

**Required**:
- Progress reporting via stdout
- Status commands
- Error handling

**Interface**:
```rust
// CLI sends messages:
sync_tx.send(SyncMessage::SyncRequest { peer_id, path }).await?;

// CLI monitors status:
let status = sync_engine.get_status().await;
println!("Progress: {:.1}%", status.progress);
```

## Error Handling

### SyncError Types

```rust
pub enum SyncError {
    Protocol(String),     // Protocol violations
    Network(String),      // QUIC transport errors  
    Storage(String),      // CAS operation errors
    Timeout,              // Operation timeouts
    Cancelled,            // User cancellation
}
```

### Error Recovery

- **Network errors**: Automatic retry with exponential backoff
- **Protocol errors**: Session termination and restart
- **Storage errors**: Chunk re-request from peer
- **Timeouts**: Progress reporting and retry options

## Testing

### Test Script

Run the end-to-end test:

```bash
./scripts/test_sync.sh
```

### Unit Tests

```bash
cargo test -p landro-sync
```

### Integration Tests

```bash 
cargo test --test integration_tests
```

## Limitations (Alpha v0.0.1)

### Current Simplifications

1. **One-way sync only**: Source → Destination
2. **No conflict resolution**: Last write wins
3. **No compression**: Raw chunk transfer  
4. **No resume**: Must restart interrupted transfers
5. **Basic manifest**: No merkle trees or signatures

### Future Enhancements (v0.0.2+)

1. **Bidirectional sync**: Automatic merge
2. **Conflict resolution**: User choice or policy
3. **Compression**: LZ4/Zstd for chunks
4. **Resume capability**: Persistent transfer state
5. **Advanced manifests**: Merkle trees, signatures
6. **Encryption**: Additional layer beyond QUIC TLS

## Memory Usage

### Protocol Specification Location

The sync protocol specification is stored in:
- **File**: `docs/sync_protocol_spec.json`
- **Memory**: `landropic/interfaces/sync_protocol`

Access via claude-flow hooks:
```bash
npx claude-flow@alpha memory get landropic/interfaces/sync_protocol
```

## Team Coordination

### Dependencies

- **QUIC Network**: Connection management, stream multiplexing  
- **Storage**: Content-addressed storage, chunk verification
- **Security**: Device pairing, mTLS certificates

### Status

✅ **Complete**: Protocol design, QUIC integration, message flow
⚠️ **Pending**: End-to-end testing requires network layer
🔄 **In Progress**: Performance optimization, error handling

### Next Steps

1. **Network Engineer**: Complete QUIC connection establishment
2. **Integration**: Connect sync transport to daemon
3. **Testing**: End-to-end sync validation
4. **Documentation**: Update user guides

---

**Contact**: Sync Engineer via claude-flow hooks
**Last Updated**: Alpha Sprint Day 1
**Status**: ✅ Ready for integration