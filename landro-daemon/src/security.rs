//! Security utilities for landropic daemon
//! 
//! This module provides basic security features for the alpha release,
//! including path traversal protection and input validation.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Maximum allowed file size for sync (16MB for alpha)
pub const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Maximum path depth to prevent deep directory traversal
pub const MAX_PATH_DEPTH: usize = 32;

/// Maximum filename length
pub const MAX_FILENAME_LENGTH: usize = 255;

/// Validates and canonicalizes a path to prevent traversal attacks
/// 
/// # Arguments
/// * `base` - The base directory that must contain the path
/// * `requested` - The requested path (may be relative)
/// 
/// # Returns
/// A canonicalized path that is guaranteed to be within the base directory
/// 
/// # Security
/// - Resolves symlinks to prevent symlink attacks
/// - Ensures the final path is within the base directory
/// - Rejects paths containing ".." components after canonicalization
pub fn validate_path(base: &Path, requested: &Path) -> Result<PathBuf> {
    // First, get the canonical base path
    let canonical_base = base.canonicalize()
        .map_err(|e| anyhow!("Failed to canonicalize base path {}: {}", base.display(), e))?;
    
    // Combine base and requested path
    let combined = if requested.is_absolute() {
        // Reject absolute paths that try to escape
        return Err(anyhow!("Absolute paths not allowed: {}", requested.display()));
    } else {
        canonical_base.join(requested)
    };
    
    // Canonicalize the combined path (resolves symlinks and "..")
    let canonical = combined.canonicalize()
        .map_err(|e| anyhow!("Failed to canonicalize path {}: {}", combined.display(), e))?;
    
    // Verify the canonical path starts with the base path
    if !canonical.starts_with(&canonical_base) {
        warn!("Path traversal attempt detected: {} -> {}", 
              requested.display(), canonical.display());
        return Err(anyhow!("Path traversal attempt: path escapes base directory"));
    }
    
    // Check path depth
    let depth = canonical.components().count();
    if depth > MAX_PATH_DEPTH {
        return Err(anyhow!("Path too deep: {} levels (max {})", depth, MAX_PATH_DEPTH));
    }
    
    // Check filename length
    if let Some(filename) = canonical.file_name() {
        if filename.len() > MAX_FILENAME_LENGTH {
            return Err(anyhow!("Filename too long: {} bytes (max {})", 
                             filename.len(), MAX_FILENAME_LENGTH));
        }
    }
    
    debug!("Path validated: {} -> {}", requested.display(), canonical.display());
    Ok(canonical)
}

/// Validates chunk size to prevent resource exhaustion
pub fn validate_chunk_size(size: usize) -> Result<()> {
    if size == 0 {
        return Err(anyhow!("Chunk size cannot be zero"));
    }
    
    if size > MAX_CHUNK_SIZE {
        return Err(anyhow!("Chunk size {} exceeds maximum {} bytes", 
                         size, MAX_CHUNK_SIZE));
    }
    
    Ok(())
}

/// Validates a device name for safety
pub fn validate_device_name(name: &str) -> Result<()> {
    // Check length
    if name.is_empty() {
        return Err(anyhow!("Device name cannot be empty"));
    }
    
    if name.len() > 64 {
        return Err(anyhow!("Device name too long: {} chars (max 64)", name.len()));
    }
    
    // Check for dangerous characters
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(anyhow!("Device name contains invalid characters"));
    }
    
    // Check for control characters
    if name.chars().any(|c| c.is_control()) {
        return Err(anyhow!("Device name contains control characters"));
    }
    
    Ok(())
}

/// Sanitizes a string for safe logging (removes control chars)
pub fn sanitize_for_log(input: &str) -> String {
    input.chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .take(1000) // Limit length for logs
        .collect()
}

/// Checks if a path points to a sensitive system location
pub fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();
    
    // Check for common sensitive directories
    let sensitive_patterns = [
        "/etc", "/sys", "/proc", "/dev",
        "/.ssh", "/.gnupg", "/.aws", "/.kube",
        "/private/etc", "/system", "/library/keychains",
        "c:\\windows\\system32", "c:\\program files",
    ];
    
    for pattern in &sensitive_patterns {
        if path_str.contains(pattern) {
            return true;
        }
    }
    
    // Check for hidden files (start with .)
    if let Some(filename) = path.file_name() {
        if filename.to_string_lossy().starts_with('.') {
            return true;
        }
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[test]
    fn test_validate_path_normal() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        
        // Normal file path should work
        let result = validate_path(base, Path::new("test.txt"));
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_validate_path_traversal() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        
        // Path traversal attempt should fail
        let result = validate_path(base, Path::new("../../../etc/passwd"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }
    
    #[test]
    fn test_validate_path_absolute() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        
        // Absolute path should fail
        let result = validate_path(base, Path::new("/etc/passwd"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Absolute"));
    }
    
    #[test]
    fn test_validate_chunk_size() {
        // Valid size
        assert!(validate_chunk_size(1024).is_ok());
        
        // Zero size
        assert!(validate_chunk_size(0).is_err());
        
        // Too large
        assert!(validate_chunk_size(MAX_CHUNK_SIZE + 1).is_err());
    }
    
    #[test]
    fn test_validate_device_name() {
        // Valid names
        assert!(validate_device_name("my-device").is_ok());
        assert!(validate_device_name("Device_123").is_ok());
        
        // Invalid names
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("my/device").is_err());
        assert!(validate_device_name("my\\device").is_err());
        assert!(validate_device_name("my\0device").is_err());
        assert!(validate_device_name(&"a".repeat(100)).is_err());
    }
    
    #[test]
    fn test_sanitize_for_log() {
        assert_eq!(sanitize_for_log("normal text"), "normal text");
        assert_eq!(sanitize_for_log("text\0with\x01control"), "textwithcontrol");
        assert_eq!(sanitize_for_log("text\nwith\nnewlines"), "text\nwith\nnewlines");
    }
    
    #[test]
    fn test_is_sensitive_path() {
        assert!(is_sensitive_path(Path::new("/etc/passwd")));
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("C:\\Windows\\System32\\config")));
        assert!(!is_sensitive_path(Path::new("/home/user/documents/file.txt")));
    }
}