//! Input validation for CAS operations

use std::path::{Path, Component};
use thiserror::Error;

/// Maximum object size (100 MB)
pub const MAX_OBJECT_SIZE: usize = 100 * 1024 * 1024;

/// Maximum path depth for storage paths
pub const MAX_PATH_DEPTH: usize = 10;

/// CAS validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Object size {size} exceeds maximum {max}")]
    ObjectTooLarge { size: usize, max: usize },
    
    #[error("Invalid hash format: {0}")]
    InvalidHash(String),
    
    #[error("Path traversal attempt detected: {0}")]
    PathTraversal(String),
    
    #[error("Path too deep: {depth} levels (max: {max})")]
    PathTooDeep { depth: usize, max: usize },
    
    #[error("Invalid path component: {0}")]
    InvalidPathComponent(String),
}

/// Validate object size
pub fn validate_object_size(size: usize) -> Result<(), ValidationError> {
    if size > MAX_OBJECT_SIZE {
        return Err(ValidationError::ObjectTooLarge {
            size,
            max: MAX_OBJECT_SIZE,
        });
    }
    Ok(())
}

/// Validate a content hash
pub fn validate_content_hash(hash: &str) -> Result<(), ValidationError> {
    // Blake3 hash is 32 bytes = 64 hex characters
    if hash.len() != 64 {
        return Err(ValidationError::InvalidHash(
            format!("Invalid length: {} (expected 64)", hash.len())
        ));
    }
    
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError::InvalidHash(
            "Not valid hexadecimal".to_string()
        ));
    }
    
    Ok(())
}

/// Validate a storage path to prevent traversal attacks
pub fn validate_storage_path(path: &Path) -> Result<(), ValidationError> {
    let mut depth = 0;
    
    for component in path.components() {
        depth += 1;
        if depth > MAX_PATH_DEPTH {
            return Err(ValidationError::PathTooDeep {
                depth,
                max: MAX_PATH_DEPTH,
            });
        }
        
        match component {
            Component::Normal(name) => {
                let name_str = name.to_string_lossy();
                
                // Reject hidden files (starting with .)
                if name_str.starts_with('.') {
                    return Err(ValidationError::InvalidPathComponent(
                        format!("Hidden files not allowed: {}", name_str)
                    ));
                }
                
                // Reject special characters that could cause issues
                if name_str.contains('\0') {
                    return Err(ValidationError::InvalidPathComponent(
                        "Null bytes not allowed in path".to_string()
                    ));
                }
            }
            Component::ParentDir => {
                return Err(ValidationError::PathTraversal(
                    "Parent directory references not allowed".to_string()
                ));
            }
            Component::RootDir => {
                return Err(ValidationError::PathTraversal(
                    "Absolute paths not allowed".to_string()
                ));
            }
            Component::CurDir => {
                // Current directory is allowed
            }
            Component::Prefix(_) => {
                return Err(ValidationError::PathTraversal(
                    "Windows path prefixes not allowed".to_string()
                ));
            }
        }
    }
    
    Ok(())
}

/// Validate that a path is within a root directory
pub fn validate_path_within_root(path: &Path, root: &Path) -> Result<(), ValidationError> {
    // Canonicalize paths for comparison
    let canonical_path = path.canonicalize()
        .map_err(|_| ValidationError::InvalidPathComponent("Cannot canonicalize path".to_string()))?;
    let canonical_root = root.canonicalize()
        .map_err(|_| ValidationError::InvalidPathComponent("Cannot canonicalize root".to_string()))?;
    
    // Check if path starts with root
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ValidationError::PathTraversal(
            format!("Path escapes root directory: {:?}", canonical_path)
        ));
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_validate_object_size() {
        assert!(validate_object_size(1024).is_ok());
        assert!(validate_object_size(MAX_OBJECT_SIZE).is_ok());
        assert!(validate_object_size(MAX_OBJECT_SIZE + 1).is_err());
    }
    
    #[test]
    fn test_validate_content_hash() {
        // Valid 64-char hex hash
        let valid = "a".repeat(64);
        assert!(validate_content_hash(&valid).is_ok());
        
        // Invalid cases
        assert!(validate_content_hash("").is_err());
        assert!(validate_content_hash("short").is_err());
        assert!(validate_content_hash(&"x".repeat(63)).is_err());
        assert!(validate_content_hash(&"x".repeat(65)).is_err());
        assert!(validate_content_hash(&"g".repeat(64)).is_err()); // Not hex
    }
    
    #[test]
    fn test_validate_storage_path() {
        use std::path::PathBuf;
        
        // Valid paths
        assert!(validate_storage_path(&PathBuf::from("file.txt")).is_ok());
        assert!(validate_storage_path(&PathBuf::from("dir/file.txt")).is_ok());
        
        // Invalid paths
        assert!(validate_storage_path(&PathBuf::from("../parent")).is_err());
        assert!(validate_storage_path(&PathBuf::from("/absolute")).is_err());
        assert!(validate_storage_path(&PathBuf::from(".hidden")).is_err());
        
        // Too deep
        let deep = "a/".repeat(20) + "file.txt";
        assert!(validate_storage_path(&PathBuf::from(deep)).is_err());
    }
}