# Landropic Protocols and Security

## Overview

Landropic implements a multi-layered security architecture with custom protocols designed for secure, efficient file synchronization on local networks. This document details the network protocols, cryptographic design, and security model.

## Security Architecture

### Defense-in-Depth Approach

Landropic employs multiple security layers:

```
┌─────────────────────────────────────┐
│        Application Layer            │ ← File encryption, access control
├─────────────────────────────────────┤
│      Landropic Protocol Layer       │ ← Custom sync protocol, integrity
├─────────────────────────────────────┤
│      QUIC Transport Security        │ ← TLS 1.3, mTLS authentication  
├─────────────────────────────────────┤
│      Network Layer Security        │ ← Device identity, certificate pinning
├─────────────────────────────────────┤
│      Physical Network Security      │ ← LAN isolation, WPA3/WPA2
└─────────────────────────────────────┘
```

### Core Security Principles

1. **Zero Trust**: No implicit trust between devices
2. **Cryptographic Authentication**: Ed25519-based device identity
3. **End-to-End Encryption**: ChaCha20-Poly1305 content encryption
4. **Forward Secrecy**: Ephemeral key exchange for sessions
5. **Integrity Protection**: Blake3 cryptographic hashing
6. **Privacy by Design**: No metadata leakage or cloud dependencies

---

## Cryptographic Foundation

### Device Identity System

Each Landropic device has a unique, persistent identity:

```rust
// Ed25519 key pair generation
let identity = DeviceIdentity::generate("device-name")?;
let device_id = DeviceId::from_public_key(identity.public_key());

// Device ID format: Blake3(Ed25519_public_key)[0:16] (128-bit)
// Example: "a1b2c3d4e5f67890abcdef1234567890"
```

**Properties:**
- **Uniqueness**: Cryptographically unique device identifiers
- **Persistence**: Stored securely on disk with appropriate permissions  
- **Non-repudiation**: Digital signatures prove device authenticity
- **Performance**: Fast signature generation and verification

### Key Management

```
Device Identity (Long-term)
├── Ed25519 Private Key (device.key) - Stored on disk, 0600 permissions
├── Ed25519 Public Key - Embedded in certificates
└── Device ID - Blake3(public_key)[0:16]

Session Keys (Ephemeral)
├── QUIC TLS 1.3 Session Keys - Forward secrecy
├── Content Encryption Keys - Per-file ChaCha20 keys
└── MAC Keys - Blake3 keyed MAC for integrity
```

### Cryptographic Algorithms

| Purpose | Algorithm | Key Size | Notes |
|---------|-----------|----------|-------|
| Device Identity | Ed25519 | 256 bits | Long-term signing keys |
| Key Exchange | X25519 | 256 bits | Ephemeral ECDH for pairing |
| Content Encryption | ChaCha20-Poly1305 | 256 bits | AEAD for file content |
| Transport Encryption | TLS 1.3 (AES/ChaCha20) | 256 bits | QUIC transport security |
| Cryptographic Hashing | Blake3 | 256 bits | Content addressing, integrity |
| Key Derivation | Argon2id | - | Password-based key derivation |
| Message Authentication | Blake3-MAC | 256 bits | Keyed authentication |

---

## Device Pairing Protocol

### SPAKE2-Inspired PAKE (Password Authenticated Key Exchange)

The pairing process establishes mutual trust between devices using a shared passphrase:

```
Alice (Initiator)                          Bob (Responder)
-----------------                          ----------------
1. K = Argon2id(passphrase, salt)          1. K = Argon2id(passphrase, salt)
2. (a, A) = X25519.keygen()                2. Wait for Message A
3. Send: A || nonce_A                      
                                           3. (b, B) = X25519.keygen()  
                                           4. S = X25519(b, A)
                                           5. SK = S ⊕ K
                                           6. conf_B = Blake3_MAC(SK, nonce_A || B)
                                           7. Send: B || nonce_B || conf_B

8. S = X25519(a, B)                        
9. SK = S ⊕ K                              
10. Verify conf_B                          8. conf_A = Blake3_MAC(SK, nonce_B || A)
11. Send: conf_A                           9. Send: conf_A

                                           10. Verify conf_A
12. Exchange encrypted identities:         11. Exchange encrypted identities:
    Enc(SK, ID_A || Sig_A(A||B))              Enc(SK, ID_B || Sig_B(B||A))
13. Verify signature with Bob's pubkey    12. Verify signature with Alice's pubkey
14. Store Bob's identity as trusted        13. Store Alice's identity as trusted
```

### Security Properties

- **Mutual Authentication**: Both devices prove knowledge of passphrase
- **Forward Secrecy**: Ephemeral X25519 keys prevent retroactive decryption
- **Dictionary Attack Resistance**: Argon2id (64MB, 3 iterations) makes offline attacks expensive  
- **MITM Resistance**: Cannot forge confirmation without passphrase knowledge
- **Identity Binding**: Ed25519 signatures prevent identity substitution

---

## QUIC Transport Layer

### Why QUIC?

1. **Performance**: 0-RTT connection establishment, multiplexed streams
2. **Security**: Mandatory TLS 1.3 encryption  
3. **Reliability**: Built-in congestion control and loss recovery
4. **Modern Design**: Designed for security and performance from ground up

### mTLS Certificate System

Landropic uses automatic X.509 certificate generation with mutual authentication:

```rust
// Generate device certificate from Ed25519 identity
let (cert_chain, private_key) = CertificateGenerator::generate_device_certificate(&identity)?;

// Certificate contains:
// - Subject: CN=device-id
// - SAN: DNS=device-name, URI=landropic://device-id  
// - Public Key: P-256 key derived from Ed25519
// - Signature: Self-signed with derived P-256 private key
```

**Certificate Properties:**
- **Self-Signed**: No certificate authority required
- **Device Binding**: Certificate contains device ID in subject and SAN
- **Key Derivation**: P-256 keys derived deterministically from Ed25519 identity
- **Verification**: Peers verify expected device ID matches certificate

### QUIC Configuration

```rust
let config = QuicConfig::default()
    .with_port(7703)                    // Default listening port
    .with_max_streams(1000)             // Per-connection stream limit
    .with_max_concurrent_transfers(10)  // Parallel file transfers
    .with_keep_alive_interval(30_000)   // 30-second keepalive
    .with_idle_timeout(300_000)         // 5-minute idle timeout
    .with_congestion_control("bbr")     // BBR for LAN optimization
    .enable_0rtt(true);                 // 0-RTT for reconnections
```

### Stream Types and Usage

| Stream Type | Direction | Purpose | Example |
|-------------|-----------|---------|---------|
| Control | Unidirectional | Protocol handshake, manifest exchange | Device capabilities, sync negotiation |
| Data | Bidirectional | File chunk transfers | Large file uploads/downloads |
| Status | Unidirectional | Progress reporting, errors | Transfer progress, completion status |
| Metadata | Bidirectional | File metadata operations | Permission updates, timestamp sync |

---

## Landropic Sync Protocol

### Protocol Layers

```
┌─────────────────────────────────────┐
│     Application Messages            │ ← File operations, sync commands
├─────────────────────────────────────┤
│     Protocol Buffers               │ ← Message serialization
├─────────────────────────────────────┤
│     Stream Multiplexing            │ ← Control vs data stream separation
├─────────────────────────────────────┤
│     QUIC Transport                 │ ← Reliable, encrypted delivery
└─────────────────────────────────────┘
```

### Message Types (Protocol Buffers)

```protobuf
// Core message wrapper
message ProtocolMessage {
  MessageType type = 1;
  bytes payload = 2;
  uint64 sequence_id = 3;
  google.protobuf.Timestamp timestamp = 4;
}

enum MessageType {
  // Handshake and capabilities
  DEVICE_HELLO = 0;
  DEVICE_CAPABILITIES = 1;
  
  // Sync operations
  SYNC_REQUEST = 10;
  SYNC_RESPONSE = 11;
  MANIFEST_UPDATE = 12;
  
  // File operations
  CHUNK_REQUEST = 20;
  CHUNK_DATA = 21;
  FILE_METADATA = 22;
  
  // Status and control
  TRANSFER_PROGRESS = 30;
  ERROR_RESPONSE = 31;
  PING = 32;
}
```

### Sync Flow Overview

```
Device A                               Device B
--------                               --------
1. DEVICE_HELLO        -->
                       <--             2. DEVICE_CAPABILITIES
3. SYNC_REQUEST        -->
                       <--             4. SYNC_RESPONSE
5. MANIFEST_UPDATE     -->
                       <--             6. CHUNK_REQUEST
7. CHUNK_DATA          -->
                       <--             8. TRANSFER_PROGRESS
9. (Repeat 6-8 for all chunks)
10. Sync Complete
```

### Content-Defined Chunking

Files are split using FastCDC (Fast Content-Defined Chunking):

```rust
let chunker = FastCdc::new(content, 8192, 16384, 65536)?; // 8KB-64KB chunks
for chunk in chunker {
    let hash = Blake3::hash(&chunk.data);
    let object_ref = ObjectRef::from_hash(hash);
    // Store in content-addressable storage
    content_store.put(object_ref, &chunk.data).await?;
}
```

**Benefits:**
- **Deduplication**: Identical chunks stored only once
- **Efficient Sync**: Only changed chunks need transfer
- **Resumability**: Failed transfers can resume at chunk boundaries
- **Integrity**: Each chunk has cryptographic hash verification

---

## Network Discovery

### mDNS (Multicast DNS) Service Advertisement

Landropic uses Bonjour/Zeroconf for automatic device discovery:

```
Service Type: _landropic._tcp.local
Port: 7703 (or configured port)
TXT Records:
  - version=1.0
  - device_id=a1b2c3d4e5f67890abcdef1234567890
  - device_name=John's MacBook
  - capabilities=sync,transfer,chunking
  - protocol_version=2
```

### Discovery Flow

```
1. Daemon starts → Register mDNS service
2. Periodic browse for _landropic._tcp.local services
3. Resolve service → Get IP address and port
4. Attempt QUIC connection with mTLS
5. Verify device identity matches expected
6. Add to peer list if trusted
```

### Network Security Considerations

- **LAN Isolation**: Only advertises on local network interfaces
- **Firewall Integration**: Respects system firewall rules
- **Rate Limiting**: Prevents discovery spam attacks
- **IP Validation**: Rejects connections from unexpected networks

---

## Content Encryption

### File-Level Encryption

Each file is encrypted independently with ChaCha20-Poly1305:

```rust
// Per-file encryption key derivation
let file_key = Blake3::derive_key(
    b"landropic-file-encryption-2024",
    &device_identity.public_key(),
    file_path.as_bytes()
);

// Encrypt file content
let nonce = random_nonce();
let encrypted_content = ChaCha20Poly1305::encrypt(
    &file_key,
    &nonce,
    &file_content,
    &file_metadata  // Additional authenticated data
)?;
```

### Key Properties

- **Per-File Keys**: Each file encrypted with unique key
- **Deterministic**: Same file on same device always has same key
- **Forward Secrecy**: File keys independent of device identity compromise
- **Authenticated**: Metadata included in AEAD authentication
- **Efficient**: ChaCha20 optimized for software implementation

### Chunk-Level Integrity

Individual chunks are integrity-protected:

```rust
let chunk_hash = Blake3::hash(&chunk_data);
let authenticated_chunk = AuthenticatedChunk {
    data: chunk_data,
    hash: chunk_hash,
    size: chunk_data.len(),
    compression: CompressionType::Zstd,
};
```

---

## Threat Model and Security Analysis

### Threat Actors

1. **Passive Network Attacker**: Monitors network traffic
2. **Active Network Attacker**: Man-in-the-middle, packet injection
3. **Malicious Insider**: Compromised device in sync group
4. **Device Compromise**: Attacker gains access to device filesystem
5. **Infrastructure Compromise**: Router/network equipment compromise

### Security Controls

| Threat | Control | Implementation |
|--------|---------|---------------|
| Traffic Analysis | TLS 1.3 Encryption | All QUIC traffic encrypted |
| Content Disclosure | End-to-End Encryption | ChaCha20-Poly1305 file encryption |
| Device Impersonation | mTLS + Certificate Pinning | Ed25519 identity verification |
| MITM Attacks | SPAKE2 PAKE + mTLS | Authenticated key exchange |
| Replay Attacks | Sequence Numbers + Timestamps | Protocol-level replay protection |
| Dictionary Attacks | Argon2id | Expensive key derivation (64MB) |
| Data Tampering | Blake3 Integrity | Cryptographic hash verification |
| Forward Compromise | Ephemeral Keys | X25519 ECDH for sessions |

### Security Assumptions

**Trusted:**
- Device operating system and Rust runtime
- Cryptographic implementations (ring, rustls)
- Local network infrastructure (within reason)
- Physical security of paired devices

**Not Trusted:**
- Network infrastructure beyond local network
- Cloud services or external parties
- Unpaired devices on network
- Timing attack resistance (partially mitigated)

### Known Limitations

1. **Timing Attacks**: Ed25519 signatures may leak information
2. **Traffic Analysis**: Packet sizes/timing may reveal patterns
3. **Denial of Service**: No rate limiting on expensive operations
4. **Quantum Resistance**: Ed25519 and X25519 not quantum-safe
5. **Key Storage**: Device private keys stored in filesystem

---

## Implementation Security

### Memory Safety

- **Rust Language**: Memory safety and thread safety by default
- **Secret Zeroization**: `zeroize` crate clears sensitive data
- **Move Semantics**: Prevents accidental copying of secrets
- **Constant-Time Operations**: Where cryptographically relevant

### Secure Storage

```bash
# Device identity permissions (Unix)
~/.config/landropic/identity/device.key  (mode: 0600, owner-only)

# Configuration and data
~/.config/landropic/config.json         (mode: 0644, readable)
~/.local/share/landropic/objects/       (mode: 0700, owner-only)
~/.local/share/landropic/index.sqlite   (mode: 0600, owner-only)
```

### Error Handling

- **No Secret Leakage**: Errors don't expose cryptographic material
- **Fail-Safe Defaults**: Security failures terminate connections
- **Audit Logging**: Security events logged for monitoring
- **Graceful Degradation**: Maintains security under adverse conditions

---

## Compliance and Standards

### Cryptographic Standards

- **FIPS 140-2**: Uses FIPS-approved algorithms where available
- **NIST Guidelines**: Follows NIST cryptographic recommendations
- **RFC Compliance**: Implements standard protocols correctly
- **Constant-Time**: Critical operations resistant to timing attacks

### Relevant RFCs and Standards

- **RFC 9000**: QUIC Transport Protocol
- **RFC 8446**: TLS 1.3
- **RFC 8032**: Ed25519 Signature Algorithm  
- **RFC 7748**: Elliptic Curves for Security (X25519)
- **RFC 8439**: ChaCha20-Poly1305 AEAD
- **RFC 9106**: Argon2 Password Hashing
- **Draft SPAKE2**: Password Authenticated Key Exchange

### Security Certifications

While not formally certified, Landropic's design aligns with:
- **Common Criteria**: Protection profiles for network security
- **FIDO Alliance**: Passwordless authentication principles
- **OWASP**: Secure coding practices and threat modeling

---

## Monitoring and Observability

### Security Event Logging

```rust
// Example security events
tracing::warn!(
    device_id = %peer_device_id,
    error = %error,
    "Certificate verification failed for peer"
);

tracing::info!(
    device_id = %device_id,
    peer_count = peer_list.len(),
    "Device pairing completed successfully"
);
```

### Metrics Collection

- **Connection Attempts**: Success/failure rates by peer
- **Certificate Errors**: Frequency and types of TLS failures  
- **Encryption Performance**: Throughput and latency metrics
- **Protocol Violations**: Invalid messages or state transitions

### Security Monitoring

- **Anomaly Detection**: Unusual traffic patterns or peer behavior
- **Rate Limiting**: Excessive pairing attempts or connections
- **Resource Usage**: CPU/memory consumption during crypto operations
- **Network Analysis**: Unexpected protocols or destinations

---

## Future Security Enhancements

### Planned Improvements

1. **Post-Quantum Cryptography**: Migration to quantum-resistant algorithms
2. **Hardware Security Modules**: TPM/Secure Enclave integration
3. **Multi-Factor Authentication**: Additional authentication factors
4. **Formal Verification**: Mathematical proofs of protocol security
5. **Differential Privacy**: Protection against statistical analysis

### Research Areas

- **Zero-Knowledge Proofs**: Privacy-preserving authentication
- **Homomorphic Encryption**: Computation on encrypted data
- **Secure Multi-Party Computation**: Collaborative operations
- **Threshold Cryptography**: Distributed key management
- **Side-Channel Resistance**: Protection against timing/power analysis

---

## Security Testing

### Test Coverage

- **Unit Tests**: Individual cryptographic functions
- **Integration Tests**: End-to-end protocol flows
- **Fuzzing**: Random input validation
- **Penetration Testing**: Simulated attacks
- **Performance Testing**: Cryptographic operation benchmarks

### Security Validation

```bash
# Run security-focused tests
cargo test security --features security-tests

# Fuzz testing with cargo-fuzz
cargo install cargo-fuzz
cargo fuzz run protocol_parsing

# Static analysis with clippy
cargo clippy -- -D warnings -W clippy::all

# Audit dependencies
cargo audit
```

---

This comprehensive security documentation provides the foundation for understanding Landropic's security architecture. The design prioritizes security while maintaining usability and performance for local network file synchronization.