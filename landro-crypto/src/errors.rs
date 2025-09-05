use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Key generation failed: {0}")]
    KeyGeneration(String),
    
    #[error("Invalid key format: {0}")]
    InvalidKeyFormat(String),
    
    #[error("Certificate generation failed: {0}")]
    CertificateGeneration(String),
    
    #[error("Certificate validation failed: {0}")]
    CertificateValidation(String),
    
    #[error("Signature verification failed")]
    SignatureVerification,
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Key not found at path: {0}")]
    KeyNotFound(String),
}

pub type Result<T> = std::result::Result<T, CryptoError>;