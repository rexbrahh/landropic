# landro-crypto

Cryptographic primitives and identity management for Landropic, providing secure device authentication and communication.

## Overview

This crate handles all cryptographic operations for Landropic, a secure file synchronization tool. It provides:

- **Device Identity Management**: Ed25519-based device identities with secure key storage
- **Certificate Generation**: X.509 certificates for mTLS authentication
- **Trust Management**: Certificate verification and device trust relationships
- **Secure Storage**: Platform-specific secure key persistence

## Key Components

### DeviceIdentity

Manages long-term Ed25519 keypairs that uniquely identify devices in the Landropic network:

```rust
use landro_crypto::DeviceIdentity;

// Generate a new identity
let identity = DeviceIdentity::generate("Alice's MacBook").unwrap();
println!("Device ID: {}", identity.device_id());

// Sign a message for authentication
let message = b"sync request from Alice";
let signature = identity.sign(message);

// Verify signatures from other devices
DeviceIdentity::verify_signature(
    &identity.device_id(),
    message,
    &signature
).unwrap();
```

### Certificate Management

Generates self-signed TLS certificates that embed device identity information:

```rust
use landro_crypto::{DeviceIdentity, CertificateGenerator};

let identity = DeviceIdentity::generate("Bob's Phone").unwrap();

// Generate a certificate for this device
let (cert_chain, private_key) = 
    CertificateGenerator::generate_device_certificate(&identity).unwrap();

// Use with rustls/quinn for QUIC connections
```

### Trust Relationships

Manages which devices are trusted for secure connections:

```rust
use landro_crypto::{CertificateVerifier, DeviceId};

// Start with a list of trusted devices
let trusted_devices = vec![
    DeviceId::from_bytes(&alice_public_key).unwrap(),
    DeviceId::from_bytes(&bob_public_key).unwrap(),
];

let mut verifier = CertificateVerifier::new(trusted_devices);

// Add new devices after pairing
verifier.add_trusted_device(new_device_id);

// Check trust during connections
if verifier.is_trusted(&peer_device_id) {
    println!("Connection allowed");
}
```

## Security Model

### Device Identity

- **Ed25519 Keys**: Each device has a unique Ed25519 keypair
- **Device IDs**: Public keys serve as immutable device identifiers
- **Fingerprints**: Blake3-based fingerprints for human verification during pairing

### Certificate Trust

- **Self-Signed**: All certificates are self-signed (no CA hierarchy)
- **Trust on First Use**: Devices start with empty trust lists
- **Device ID Embedding**: Certificates contain device IDs in custom extensions
- **Mutual Authentication**: Both client and server verify each other

### Key Storage

- **Secure Permissions**: Keys stored with 0600 permissions on Unix systems
- **Atomic Writes**: Key files written atomically to prevent corruption
- **Memory Protection**: Sensitive data zeroized when no longer needed

## Integration with QUIC

This crate integrates with `landro-quic` to provide secure transport:

```rust
use landro_crypto::{DeviceIdentity, CertificateVerifier};
use landro_quic::{QuicClient, QuicConfig};
use std::sync::Arc;

// Load device identity
let identity = Arc::new(
    DeviceIdentity::load_or_generate("my-device").await?
);

// Configure trust relationships
let verifier = Arc::new(CertificateVerifier::new(trusted_devices));

// Create secure QUIC client
let client = QuicClient::new(identity, verifier, QuicConfig::default()).await?;
let connection = client.connect("peer.local:9876").await?;
```

## Error Handling

All operations return `Result<T, CryptoError>` with detailed error information:

- `CryptoError::InvalidKeyFormat`: Malformed keys or certificates
- `CryptoError::SignatureVerification`: Invalid digital signatures
- `CryptoError::KeyNotFound`: Missing identity files
- `CryptoError::CertificateGeneration`: X.509 generation failures
- `CryptoError::Io`: File system operations

## File Locations

By default, device identities are stored in:

- **Unix**: `~/.landropic/keys/device_identity.key`
- **Windows**: `%USERPROFILE%\.landropic\keys\device_identity.key`

The key file format includes:

- Version number for future compatibility
- Device name for identification
- Ed25519 private key (32 bytes)
- Proper serialization with bincode

## Testing

The crate includes comprehensive tests covering:

```bash
cargo test -p landro-crypto
```

- Key generation and serialization
- Signature creation and verification
- Certificate generation and parsing
- Trust relationship management
- Error condition handling

## Security Considerations

- **Key Generation**: Uses `OsRng` for cryptographically secure randomness
- **Memory Safety**: Automatic zeroization of sensitive data structures
- **File Permissions**: Restricts key file access to owner only
- **Forward Secrecy**: TLS 1.3 provides ephemeral key exchange
- **Certificate Validation**: Custom verification ensures device ID authenticity

## Dependencies

Core cryptographic dependencies:

- `ed25519-dalek`: Ed25519 signatures
- `blake3`: Fast cryptographic hashing
- `rustls`: TLS implementation
- `rcgen`: X.509 certificate generation
- `zeroize`: Memory clearing for sensitive data