use thiserror::Error;

#[derive(Error, Debug)]
pub enum QuicError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Connection error: {0}")]
    Connection(#[from] quinn::ConnectionError),

    #[error("Connect error: {0}")]
    Connect(#[from] quinn::ConnectError),

    #[error("TLS configuration error: {0}")]
    TlsConfig(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Timeout")]
    Timeout,

    #[error("Handshake timeout after {timeout_secs} seconds")]
    HandshakeTimeout { timeout_secs: u64 },

    #[error("Handshake failed: {reason}")]
    HandshakeFailed { reason: String },

    #[error("Version negotiation failed: {details}")]
    VersionMismatch { details: String },

    #[error("Authentication failed: {reason}")]
    AuthenticationFailed { reason: String },

    #[error("Server already running")]
    ServerAlreadyRunning,

    #[error("Crypto error: {0}")]
    Crypto(#[from] landro_crypto::CryptoError),
}

impl QuicError {
    /// Create a handshake failure error with context
    pub fn handshake_failed(reason: impl Into<String>) -> Self {
        Self::HandshakeFailed {
            reason: reason.into(),
        }
    }

    /// Create a version mismatch error with details
    pub fn version_mismatch(details: impl Into<String>) -> Self {
        Self::VersionMismatch {
            details: details.into(),
        }
    }

    /// Create an authentication failure error
    pub fn authentication_failed(reason: impl Into<String>) -> Self {
        Self::AuthenticationFailed {
            reason: reason.into(),
        }
    }

    /// Check if this error is recoverable (could retry)
    pub fn is_recoverable(&self) -> bool {
        match self {
            QuicError::Timeout | QuicError::HandshakeTimeout { .. } => true,
            QuicError::Io(_) => true,
            QuicError::Connect(_) => true,
            QuicError::Connection(_) => false, // Usually permanent
            QuicError::VersionMismatch { .. } => false, // Incompatible versions
            QuicError::AuthenticationFailed { .. } => false, // Auth issues are permanent
            QuicError::HandshakeFailed { .. } => false, // Protocol issues usually permanent
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, QuicError>;

// Manual Clone implementation to handle non-cloneable wrapped errors
impl Clone for QuicError {
    fn clone(&self) -> Self {
        match self {
            Self::Io(e) => Self::Protocol(format!("IO error: {}", e)),
            Self::Connection(e) => Self::Protocol(format!("Connection error: {}", e)),
            Self::Connect(e) => Self::Protocol(format!("Connect error: {}", e)),
            Self::TlsConfig(s) => Self::TlsConfig(s.clone()),
            Self::Stream(s) => Self::Stream(s.clone()),
            Self::Protocol(s) => Self::Protocol(s.clone()),
            Self::Timeout => Self::Timeout,
            Self::HandshakeTimeout { timeout_secs } => Self::HandshakeTimeout {
                timeout_secs: *timeout_secs,
            },
            Self::HandshakeFailed { reason } => Self::HandshakeFailed {
                reason: reason.clone(),
            },
            Self::VersionMismatch { details } => Self::VersionMismatch {
                details: details.clone(),
            },
            Self::AuthenticationFailed { reason } => Self::AuthenticationFailed {
                reason: reason.clone(),
            },
            Self::ServerAlreadyRunning => Self::ServerAlreadyRunning,
            Self::Crypto(e) => Self::Protocol(format!("Crypto error: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_types() {
        let handshake_err = QuicError::handshake_failed("Protocol mismatch");
        assert!(matches!(handshake_err, QuicError::HandshakeFailed { .. }));
        assert!(!handshake_err.is_recoverable());

        let version_err = QuicError::version_mismatch("Version 2.0.0 not compatible");
        assert!(matches!(version_err, QuicError::VersionMismatch { .. }));
        assert!(!version_err.is_recoverable());

        let auth_err = QuicError::authentication_failed("Invalid certificate");
        assert!(matches!(auth_err, QuicError::AuthenticationFailed { .. }));
        assert!(!auth_err.is_recoverable());
    }

    #[test]
    fn test_recoverable_errors() {
        let timeout_err = QuicError::HandshakeTimeout { timeout_secs: 10 };
        assert!(timeout_err.is_recoverable());

        let io_err = QuicError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "test",
        ));
        assert!(io_err.is_recoverable());

        let version_err = QuicError::version_mismatch("incompatible");
        assert!(!version_err.is_recoverable());
    }

    #[test]
    fn test_error_messages() {
        let handshake_err = QuicError::handshake_failed("test reason");
        assert_eq!(handshake_err.to_string(), "Handshake failed: test reason");

        let timeout_err = QuicError::HandshakeTimeout { timeout_secs: 15 };
        assert_eq!(
            timeout_err.to_string(),
            "Handshake timeout after 15 seconds"
        );
    }
}
