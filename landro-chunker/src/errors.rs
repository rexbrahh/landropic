use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChunkerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    
    #[error("Chunk size out of bounds: {0}")]
    ChunkSizeOutOfBounds(usize),
    
    #[error("Stream ended unexpectedly")]
    UnexpectedEof,
}

pub type Result<T> = std::result::Result<T, ChunkerError>;