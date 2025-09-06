//! Input validation and size limits for protocol messages
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::missing_errors_doc)]

use thiserror::Error;

/// Maximum size limits for protocol messages and data
pub mod limits {
    /// Maximum size for a single protocol message (10 MB)
    pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

    /// Maximum size for a chunk (16 MB)
    pub const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;

    /// Minimum size for a chunk (256 bytes)
    pub const MIN_CHUNK_SIZE: usize = 256;

    /// Maximum length for a file path (4096 bytes)
    pub const MAX_PATH_LENGTH: usize = 4096;

    /// Maximum length for a device name (256 bytes)
    pub const MAX_DEVICE_NAME_LENGTH: usize = 256;

    /// Maximum number of files in a manifest (1 million)
    pub const MAX_MANIFEST_FILES: usize = 1_000_000;

    /// Maximum number of chunks per file (100k chunks = ~1.6TB at 16MB per chunk)
    pub const MAX_CHUNKS_PER_FILE: usize = 100_000;

    /// Maximum number of concurrent transfers
    pub const MAX_CONCURRENT_TRANSFERS: usize = 100;

    /// Maximum number of peers
    pub const MAX_PEERS: usize = 1000;

    /// Maximum folder ID length
    pub const MAX_FOLDER_ID_LENGTH: usize = 64;

    /// Maximum hash length (Blake3 = 32 bytes hex = 64 chars)
    pub const MAX_HASH_LENGTH: usize = 64;

    /// Maximum metadata size (1 MB)
    pub const MAX_METADATA_SIZE: usize = 1024 * 1024;

    /// Maximum pairing code length
    pub const MAX_PAIRING_CODE_LENGTH: usize = 512;

    /// Maximum capabilities string length
    pub const MAX_CAPABILITY_LENGTH: usize = 256;

    /// Maximum number of capabilities per device
    pub const MAX_CAPABILITIES: usize = 100;
}

/// Validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Message size {size} exceeds maximum {max}")]
    MessageTooLarge { size: usize, max: usize },

    #[error("Chunk size {size} is invalid (min: {min}, max: {max})")]
    InvalidChunkSize { size: usize, min: usize, max: usize },

    #[error("Path length {length} exceeds maximum {max}")]
    PathTooLong { length: usize, max: usize },

    #[error("Device name length {length} exceeds maximum {max}")]
    DeviceNameTooLong { length: usize, max: usize },

    #[error("Manifest has {count} files, exceeds maximum {max}")]
    ManifestTooLarge { count: usize, max: usize },

    #[error("File has {count} chunks, exceeds maximum {max}")]
    TooManyChunks { count: usize, max: usize },

    #[error("Invalid folder ID length: {length} (max: {max})")]
    InvalidFolderIdLength { length: usize, max: usize },

    #[error("Invalid hash format: {reason}")]
    InvalidHash { reason: String },

    #[error("Metadata size {size} exceeds maximum {max}")]
    MetadataTooLarge { size: usize, max: usize },

    #[error("Invalid path: {reason}")]
    InvalidPath { reason: String },

    #[error("Invalid UTF-8 in {field}")]
    InvalidUtf8 { field: String },

    #[error("Missing required field: {field}")]
    MissingField { field: String },

    #[error("Invalid protocol version: {version}")]
    InvalidVersion { version: String },

    #[error("Too many capabilities: {count} (max: {max})")]
    TooManyCapabilities { count: usize, max: usize },
}

/// Validator for protocol messages and data
pub struct Validator;

impl Validator {
    /// Validate a message size
    pub fn validate_message_size(size: usize) -> Result<(), ValidationError> {
        if size > limits::MAX_MESSAGE_SIZE {
            return Err(ValidationError::MessageTooLarge {
                size,
                max: limits::MAX_MESSAGE_SIZE,
            });
        }
        Ok(())
    }

    /// Validate a chunk size
    pub fn validate_chunk_size(size: usize) -> Result<(), ValidationError> {
        if size < limits::MIN_CHUNK_SIZE || size > limits::MAX_CHUNK_SIZE {
            return Err(ValidationError::InvalidChunkSize {
                size,
                min: limits::MIN_CHUNK_SIZE,
                max: limits::MAX_CHUNK_SIZE,
            });
        }
        Ok(())
    }

    /// Validate a file path
    pub fn validate_path(path: &str) -> Result<(), ValidationError> {
        // Check length
        if path.len() > limits::MAX_PATH_LENGTH {
            return Err(ValidationError::PathTooLong {
                length: path.len(),
                max: limits::MAX_PATH_LENGTH,
            });
        }

        // Check for null bytes
        if path.contains('\0') {
            return Err(ValidationError::InvalidPath {
                reason: "contains null bytes".to_string(),
            });
        }

        // Check for directory traversal attempts
        if path.contains("..") {
            return Err(ValidationError::InvalidPath {
                reason: "contains directory traversal".to_string(),
            });
        }

        // Check for absolute paths (we only sync relative paths)
        if path.starts_with('/') || path.starts_with('\\') {
            return Err(ValidationError::InvalidPath {
                reason: "absolute paths not allowed".to_string(),
            });
        }

        Ok(())
    }

    /// Validate a device name
    pub fn validate_device_name(name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::MissingField {
                field: "device_name".to_string(),
            });
        }

        if name.len() > limits::MAX_DEVICE_NAME_LENGTH {
            return Err(ValidationError::DeviceNameTooLong {
                length: name.len(),
                max: limits::MAX_DEVICE_NAME_LENGTH,
            });
        }

        // Check for control characters
        if name.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidUtf8 {
                field: "device_name".to_string(),
            });
        }

        Ok(())
    }

    /// Validate a folder ID
    pub fn validate_folder_id(id: &str) -> Result<(), ValidationError> {
        if id.is_empty() {
            return Err(ValidationError::MissingField {
                field: "folder_id".to_string(),
            });
        }

        if id.len() > limits::MAX_FOLDER_ID_LENGTH {
            return Err(ValidationError::InvalidFolderIdLength {
                length: id.len(),
                max: limits::MAX_FOLDER_ID_LENGTH,
            });
        }

        // Should be alphanumeric + hyphen/underscore
        if !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ValidationError::InvalidPath {
                reason: "folder ID contains invalid characters".to_string(),
            });
        }

        Ok(())
    }

    /// Validate a content hash
    pub fn validate_hash(hash: &str) -> Result<(), ValidationError> {
        if hash.is_empty() {
            return Err(ValidationError::InvalidHash {
                reason: "empty hash".to_string(),
            });
        }

        if hash.len() != 64 {
            return Err(ValidationError::InvalidHash {
                reason: format!("invalid length: {} (expected 64)", hash.len()),
            });
        }

        // Must be valid hex
        if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidHash {
                reason: "not valid hexadecimal".to_string(),
            });
        }

        Ok(())
    }

    /// Validate manifest file count
    pub fn validate_manifest_size(file_count: usize) -> Result<(), ValidationError> {
        if file_count > limits::MAX_MANIFEST_FILES {
            return Err(ValidationError::ManifestTooLarge {
                count: file_count,
                max: limits::MAX_MANIFEST_FILES,
            });
        }
        Ok(())
    }

    /// Validate chunks per file
    pub fn validate_chunk_count(chunk_count: usize) -> Result<(), ValidationError> {
        if chunk_count > limits::MAX_CHUNKS_PER_FILE {
            return Err(ValidationError::TooManyChunks {
                count: chunk_count,
                max: limits::MAX_CHUNKS_PER_FILE,
            });
        }
        Ok(())
    }

    /// Validate capabilities list
    pub fn validate_capabilities(capabilities: &[String]) -> Result<(), ValidationError> {
        if capabilities.len() > limits::MAX_CAPABILITIES {
            return Err(ValidationError::TooManyCapabilities {
                count: capabilities.len(),
                max: limits::MAX_CAPABILITIES,
            });
        }

        for cap in capabilities {
            if cap.len() > limits::MAX_CAPABILITY_LENGTH {
                return Err(ValidationError::InvalidUtf8 {
                    field: format!("capability '{}'", cap),
                });
            }
        }

        Ok(())
    }

    /// Validate a pairing code
    pub fn validate_pairing_code(code: &str) -> Result<(), ValidationError> {
        if code.is_empty() {
            return Err(ValidationError::MissingField {
                field: "pairing_code".to_string(),
            });
        }

        if code.len() > limits::MAX_PAIRING_CODE_LENGTH {
            return Err(ValidationError::InvalidUtf8 {
                field: "pairing_code".to_string(),
            });
        }

        Ok(())
    }
}

/// Validate Hello message
pub fn validate_hello(hello: &crate::Hello) -> Result<(), ValidationError> {
    // Validate version
    if hello.version.is_empty() {
        return Err(ValidationError::MissingField {
            field: "version".to_string(),
        });
    }

    if !crate::VersionNegotiator::is_compatible(&hello.version) {
        return Err(ValidationError::InvalidVersion {
            version: hello.version.clone(),
        });
    }

    // Validate device name
    Validator::validate_device_name(&hello.device_name)?;

    // Validate device ID (should be 32 bytes)
    if hello.device_id.len() != 32 {
        return Err(ValidationError::InvalidHash {
            reason: format!("Device ID must be 32 bytes, got {}", hello.device_id.len()),
        });
    }

    // Validate capabilities
    Validator::validate_capabilities(&hello.capabilities)?;

    Ok(())
}

/// Validate FolderSummary message
pub fn validate_folder_summary(summary: &crate::FolderSummary) -> Result<(), ValidationError> {
    // Validate folder ID
    Validator::validate_folder_id(&summary.folder_id)?;

    // Validate manifest hash if present (should be 32 bytes when set)
    if !summary.manifest_hash.is_empty() {
        if summary.manifest_hash.len() != 32 {
            return Err(ValidationError::InvalidHash {
                reason: format!(
                    "Manifest hash must be 32 bytes, got {}",
                    summary.manifest_hash.len()
                ),
            });
        }
    }

    // Validate file count
    Validator::validate_manifest_size(summary.file_count as usize)?;

    Ok(())
}

/// Validate Want message
pub fn validate_want(want: &crate::Want) -> Result<(), ValidationError> {
    // Validate folder ID
    Validator::validate_folder_id(&want.folder_id)?;

    // Validate each chunk hash (should be 32 bytes)
    for hash in &want.chunk_hashes {
        if hash.len() != 32 {
            return Err(ValidationError::InvalidHash {
                reason: format!("Chunk hash must be 32 bytes, got {}", hash.len()),
            });
        }
    }

    // Check reasonable number of chunks requested
    if want.chunk_hashes.len() > 1000 {
        return Err(ValidationError::TooManyChunks {
            count: want.chunk_hashes.len(),
            max: 1000,
        });
    }

    Ok(())
}

/// Validate ChunkData message
pub fn validate_chunk_data(chunk: &crate::ChunkData) -> Result<(), ValidationError> {
    // Validate hash is 32 bytes
    if chunk.hash.len() != 32 {
        return Err(ValidationError::InvalidHash {
            reason: format!("Chunk hash must be 32 bytes, got {}", chunk.hash.len()),
        });
    }

    // Validate chunk size
    Validator::validate_chunk_size(chunk.data.len())?;

    // Verify hash matches data
    let computed_hash = blake3::hash(&chunk.data);

    if computed_hash.as_bytes() != &chunk.hash[..] {
        return Err(ValidationError::InvalidHash {
            reason: "hash doesn't match data".to_string(),
        });
    }

    Ok(())
}

/// Validate FileEntry in manifest
pub fn validate_file_entry(entry: &crate::FileEntry) -> Result<(), ValidationError> {
    // Validate path
    Validator::validate_path(&entry.path)?;

    // Validate content hash (32 bytes)
    if entry.content_hash.len() != 32 {
        return Err(ValidationError::InvalidHash {
            reason: format!(
                "Content hash must be 32 bytes, got {}",
                entry.content_hash.len()
            ),
        });
    }

    // Validate chunk count
    Validator::validate_chunk_count(entry.chunk_hashes.len())?;

    // Validate each chunk hash (32 bytes each)
    for hash in &entry.chunk_hashes {
        if hash.len() != 32 {
            return Err(ValidationError::InvalidHash {
                reason: format!("Chunk hash must be 32 bytes, got {}", hash.len()),
            });
        }
    }

    Ok(())
}

/// Validate entire Manifest
pub fn validate_manifest(manifest: &crate::Manifest) -> Result<(), ValidationError> {
    // Validate folder ID
    Validator::validate_folder_id(&manifest.folder_id)?;

    // Validate file count
    Validator::validate_manifest_size(manifest.files.len())?;

    // Validate each file entry
    for entry in &manifest.files {
        validate_file_entry(entry)?;
    }

    // Validate manifest hash (32 bytes)
    if manifest.manifest_hash.len() != 32 {
        return Err(ValidationError::InvalidHash {
            reason: format!(
                "Manifest hash must be 32 bytes, got {}",
                manifest.manifest_hash.len()
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_message_size() {
        assert!(Validator::validate_message_size(1024).is_ok());
        assert!(Validator::validate_message_size(limits::MAX_MESSAGE_SIZE).is_ok());
        assert!(Validator::validate_message_size(limits::MAX_MESSAGE_SIZE + 1).is_err());
    }

    #[test]
    fn test_validate_chunk_size() {
        assert!(Validator::validate_chunk_size(1024).is_ok());
        assert!(Validator::validate_chunk_size(limits::MIN_CHUNK_SIZE).is_ok());
        assert!(Validator::validate_chunk_size(limits::MAX_CHUNK_SIZE).is_ok());
        assert!(Validator::validate_chunk_size(limits::MIN_CHUNK_SIZE - 1).is_err());
        assert!(Validator::validate_chunk_size(limits::MAX_CHUNK_SIZE + 1).is_err());
    }

    #[test]
    fn test_validate_path() {
        assert!(Validator::validate_path("file.txt").is_ok());
        assert!(Validator::validate_path("dir/file.txt").is_ok());
        assert!(Validator::validate_path("深/nested/file.txt").is_ok());

        // Invalid paths
        assert!(Validator::validate_path("/absolute/path").is_err());
        assert!(Validator::validate_path("../traversal").is_err());
        assert!(Validator::validate_path("file\0null").is_err());
        assert!(Validator::validate_path(&"x".repeat(5000)).is_err());
    }

    #[test]
    fn test_validate_device_name() {
        assert!(Validator::validate_device_name("My Device").is_ok());
        assert!(Validator::validate_device_name("Device-123").is_ok());

        // Invalid names
        assert!(Validator::validate_device_name("").is_err());
        assert!(Validator::validate_device_name(&"x".repeat(300)).is_err());
        assert!(Validator::validate_device_name("Device\0").is_err());
    }

    #[test]
    fn test_validate_hash() {
        let valid_hash = "a".repeat(64);
        assert!(Validator::validate_hash(&valid_hash).is_ok());

        // Invalid hashes
        assert!(Validator::validate_hash("").is_err());
        assert!(Validator::validate_hash("short").is_err());
        assert!(Validator::validate_hash(&"x".repeat(63)).is_err());
        assert!(Validator::validate_hash(&"x".repeat(65)).is_err());
        assert!(Validator::validate_hash(&"g".repeat(64)).is_err()); // Not hex
    }

    #[test]
    fn test_validate_folder_id() {
        assert!(Validator::validate_folder_id("folder-123").is_ok());
        assert!(Validator::validate_folder_id("my_folder").is_ok());

        // Invalid IDs
        assert!(Validator::validate_folder_id("").is_err());
        assert!(Validator::validate_folder_id(&"x".repeat(100)).is_err());
        assert!(Validator::validate_folder_id("folder/slash").is_err());
    }
}
