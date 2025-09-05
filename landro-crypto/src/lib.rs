//! # Landropic Cryptography
//!
//! This crate provides cryptographic primitives and identity management for Landropic,
//! a secure file synchronization tool. It handles:
//!
//! - Device identity generation and management using Ed25519 keys
//! - TLS certificate generation and verification for secure QUIC connections
//! - Mutual TLS (mTLS) setup with device ID verification
//! - Secure key storage with appropriate file permissions
//!
//! ## Key Components
//!
//! - [`DeviceIdentity`]: Long-term Ed25519 identity keys for devices
//! - [`CertificateGenerator`]: Generate self-signed TLS certificates for mTLS
//! - [`CertificateVerifier`]: Verify peer certificates during handshakes
//!
//! ## Usage Example
//!
//! ```rust
//! use landro_crypto::{DeviceIdentity, CertificateGenerator};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Generate or load a device identity
//! let identity = DeviceIdentity::load_or_generate("my-device").await?;
//! println!("Device ID: {}", identity.device_id());
//!
//! // Generate a TLS certificate for this device
//! let (cert_chain, private_key) =
//!     CertificateGenerator::generate_device_certificate(&identity)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Security Features
//!
//! - Ed25519 signatures for device authentication
//! - Self-signed certificates with device ID embedding
//! - Secure key storage with restricted file permissions (Unix)
//! - Zeroization of sensitive data in memory
//! - Blake3-based device ID fingerprinting

pub mod certificate;
pub mod errors;
pub mod identity;
// pub mod secure_verifier; // Temporarily disabled due to compilation issues

pub use certificate::{CertificateGenerator, CertificateVerifier};
pub use errors::{CryptoError, Result};
pub use identity::{DeviceId, DeviceIdentity};
