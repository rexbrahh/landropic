#![doc = include_str!("../README.md")]

pub mod generated {
    include!("generated/landro.proto.rs");
}

pub use generated::*;

// Re-export commonly used types
pub use generated::{
    Ack, ChunkData, Error as ProtoError, FileEntry, FolderSummary, Hello, Manifest, Want,
};

// Protocol version for compatibility checking
pub const PROTOCOL_VERSION: &str = "0.1.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_version() {
        assert_eq!(PROTOCOL_VERSION, "0.1.0");
    }
}
