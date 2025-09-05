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

    #[error("Server already running")]
    ServerAlreadyRunning,

    #[error("Crypto error: {0}")]
    Crypto(#[from] landro_crypto::CryptoError),
}

pub type Result<T> = std::result::Result<T, QuicError>;
