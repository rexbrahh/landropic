//! Error types for cryptographic operations in Landropic.
//!
//! This module defines all the error conditions that can occur during
//! device identity management, certificate operations, and cryptographic
//! verification processes.

use thiserror::Error;

/// Errors that can occur during cryptographic operations.
///
/// These errors cover the full range of issues that might arise when working
/// with device identities, certificates, and cryptographic verification in Landropic.
#[derive(Error, Debug)]
pub enum CryptoError {
    /// I/O error when reading/writing key files or certificates.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to generate cryptographic keys.
    #[error("Key generation failed: {0}")]
    KeyGeneration(String),

    /// Invalid key format or size (e.g., wrong number of bytes for Ed25519 key).
    #[error("Invalid key format: {0}")]
    InvalidKeyFormat(String),

    /// Failed to generate X.509 certificates.
    #[error("Certificate generation failed: {0}")]
    CertificateGeneration(String),

    /// Certificate validation failed during TLS handshake.
    #[error("Certificate validation failed: {0}")]
    CertificateValidation(String),

    /// Digital signature verification failed.
    #[error("Signature verification failed")]
    SignatureVerification,

    /// Failed to serialize/deserialize cryptographic data.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Device identity key file not found at expected location.
    #[error("Key not found at path: {0}")]
    KeyNotFound(String),

    /// Certificate storage or retrieval error.
    #[error("Certificate storage error: {0}")]
    Storage(String),

    /// Certificate trust relationship error.
    #[error("Certificate trust error: {0}")]
    TrustError(String),
}

/// Convenient Result type alias for cryptographic operations.
pub type Result<T> = std::result::Result<T, CryptoError>;
