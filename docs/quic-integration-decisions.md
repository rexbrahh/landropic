# QUIC Transport Integration Decisions

## Overview
This document outlines the key decisions made when integrating the QUIC transport layer into the landropic daemon orchestrator.

## Architecture Decisions

### 1. Integration Approach
- **Decision**: Integrate QUIC directly into the orchestrator rather than as a separate service
- **Rationale**: 
  - Reduces inter-process communication overhead
  - Simplifies state management between sync operations and network connections
  - Enables direct access to content store and indexer

### 2. Connection Management
- **Decision**: Use connection pooling with automatic health checks
- **Rationale**:
  - Reduces connection establishment overhead for frequent sync operations
  - Provides automatic failover and recovery
  - Optimizes for LAN environments with stable peers

### 3. Stream Multiplexing Strategy
- **Decision**: Separate control and data streams
  - Control streams (unidirectional): Protocol handshake, manifest exchange, sync negotiation
  - Data streams (bidirectional): Chunk transfers, bulk data operations
- **Rationale**:
  - Prevents head-of-line blocking between control and data
  - Allows parallel chunk transfers while maintaining control flow
  - Simplifies protocol implementation

## Performance Optimizations

### 1. QUIC Configuration
- **File Sync Profile**: Optimized for bulk transfers
  ```rust
  stream_receive_window: 128MB
  receive_window: 2GB
  max_concurrent_bidi_streams: 1024
  congestion_control: BBR
  ```
- **Rationale**: Maximizes throughput for large file transfers in LAN environment

### 2. Connection Pooling
- **Configuration**:
  - Max connections per peer: 4
  - Connection idle timeout: 300s
  - Health check interval: 30s
- **Rationale**: Balances resource usage with transfer parallelism

### 3. Error Recovery
- **Exponential Backoff**: For peer connection failures
- **Handshake Timeout**: 10 seconds for protocol negotiation
- **Graceful Degradation**: Continue operation despite individual connection failures

## Security Considerations

### 1. Mutual TLS Authentication
- **Decision**: Require mTLS for all connections
- **Implementation**: 
  - Device identity loaded from persistent storage
  - Certificate verification using CertificateVerifier
  - Automatic certificate generation for new devices

### 2. Connection Security
- **Close Codes**:
  - 1: Handshake failed
  - 2: Handshake timeout
  - 3: Protocol error
- **Rationale**: Clear error signaling for debugging and security monitoring

## Integration Points

### 1. Orchestrator Initialization
```rust
orchestrator.initialize_quic_transport(bind_addr).await
```
- Creates QUIC server
- Initializes connection manager
- Starts accepting connections

### 2. Peer Discovery Integration
- Automatic connection establishment on peer discovery
- Fallback to callback mechanism if QUIC unavailable
- Sync initiation through established connections

### 3. Stream Handling
- Dedicated handlers for control and data streams
- Async processing with proper error boundaries
- Resource cleanup on connection close

## Future Improvements

### 1. Protocol Enhancements
- [ ] Implement delta sync for incremental transfers
- [ ] Add compression for manifest exchange
- [ ] Support for resumable transfers

### 2. Performance Optimizations
- [ ] Implement zero-copy transfers using sendfile
- [ ] Add adaptive congestion control
- [ ] Optimize for WAN scenarios

### 3. Monitoring & Observability
- [ ] Add metrics for connection health
- [ ] Track transfer performance statistics
- [ ] Implement connection pool monitoring

## Testing Strategy

### 1. Unit Tests
- Connection establishment and teardown
- Stream multiplexing
- Error recovery mechanisms

### 2. Integration Tests
- End-to-end sync scenarios
- Multi-peer synchronization
- Network failure recovery

### 3. Performance Tests
- Throughput benchmarks
- Latency measurements
- Concurrent connection limits

## Dependencies
- `quinn`: QUIC implementation
- `rustls`: TLS provider
- `landro-crypto`: Device identity and certificates
- `landro-quic`: QUIC transport abstractions

## Team Coordination Notes
- Network Specialist: Optimize QUIC parameters for specific network conditions
- Security Engineer: Review mTLS implementation and certificate handling
- QA Validator: Test connection reliability and error recovery
- Junior Engineer: Implement additional stream handlers as needed