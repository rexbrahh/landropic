//! Atomic file operations for crash safety

use blake3::Hasher;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::errors::{CasError, Result};

/// Atomic file writer with crash safety guarantees
pub struct AtomicWriter {
    final_path: PathBuf,
    temp_path: PathBuf,
    file: Option<fs::File>,
    hasher: Hasher,
    bytes_written: u64,
}

impl AtomicWriter {
    /// Create a new atomic writer for the given path
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let final_path = path.as_ref().to_path_buf();
        let temp_path = final_path.with_extension(format!(
            "{}.tmp.{}",
            final_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
            uuid::Uuid::new_v4().simple()
        ));

        // Ensure parent directory exists
        if let Some(parent) = temp_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Create temporary file
        let file = fs::File::create(&temp_path).await?;

        Ok(Self {
            final_path,
            temp_path,
            file: Some(file),
            hasher: Hasher::new(),
            bytes_written: 0,
        })
    }

    /// Write data to the atomic writer
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        if let Some(ref mut file) = self.file {
            file.write_all(data).await?;
            self.hasher.update(data);
            self.bytes_written += data.len() as u64;
            debug!("Wrote {} bytes to atomic writer", data.len());
            Ok(())
        } else {
            Err(CasError::InvalidOperation(
                "Writer already finalized".to_string(),
            ))
        }
    }

    /// Flush data to disk (checkpoint)
    pub async fn flush(&mut self) -> Result<()> {
        if let Some(ref mut file) = self.file {
            file.flush().await?;
            file.sync_all().await?;
            debug!("Flushed {} bytes to disk", self.bytes_written);
            Ok(())
        } else {
            Err(CasError::InvalidOperation(
                "Writer already finalized".to_string(),
            ))
        }
    }

    /// Finalize the write atomically
    pub async fn commit(mut self) -> Result<AtomicWriteResult> {
        if let Some(mut file) = self.file.take() {
            // Final flush and sync
            file.flush().await?;
            file.sync_all().await?;
            drop(file);

            // Atomic move from temp to final location
            fs::rename(&self.temp_path, &self.final_path).await?;

            let hash = self.hasher.finalize();
            debug!(
                "Committed atomic write: {} bytes, hash: {}",
                self.bytes_written,
                hash.to_hex()
            );

            Ok(AtomicWriteResult {
                path: self.final_path,
                bytes_written: self.bytes_written,
                content_hash: hash.into(),
            })
        } else {
            Err(CasError::InvalidOperation(
                "Writer already finalized".to_string(),
            ))
        }
    }

    /// Abort the write and clean up
    pub async fn abort(mut self) -> Result<()> {
        if self.file.take().is_some() {
            // Close file and remove temp file
            if self.temp_path.exists() {
                if let Err(e) = fs::remove_file(&self.temp_path).await {
                    warn!(
                        "Failed to cleanup temp file {}: {}",
                        self.temp_path.display(),
                        e
                    );
                }
            }
        }
        Ok(())
    }

    /// Get current write progress
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Get current hash (partial)
    pub fn current_hash(&self) -> blake3::Hash {
        self.hasher.finalize()
    }
}

/// Result of successful atomic write
#[derive(Debug, Clone)]
pub struct AtomicWriteResult {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub content_hash: blake3::Hash,
}

/// Check if a file write was interrupted and can be resumed
pub async fn check_resumable_write(path: &Path) -> Option<ResumableWrite> {
    // Look for temp files with our pattern
    if let Some(parent) = path.parent() {
        if let Ok(mut entries) = fs::read_dir(parent).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let entry_path = entry.path();
                if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(&format!(
                        "{}.tmp.",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("")
                    )) {
                        // Found a temp file that might be resumable
                        if let Ok(metadata) = entry.metadata().await {
                            return Some(ResumableWrite {
                                original_path: path.to_path_buf(),
                                temp_path: entry_path,
                                bytes_written: metadata.len(),
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

/// Information about a resumable write operation
#[derive(Debug, Clone)]
pub struct ResumableWrite {
    pub original_path: PathBuf,
    pub temp_path: PathBuf,
    pub bytes_written: u64,
}

impl ResumableWrite {
    /// Resume writing from where we left off
    pub async fn resume(self) -> Result<AtomicWriter> {
        // Verify temp file integrity
        let existing_data = fs::read(&self.temp_path).await?;

        if existing_data.len() != self.bytes_written as usize {
            warn!("Temp file size mismatch, starting fresh write");
            fs::remove_file(&self.temp_path).await.ok();
            return AtomicWriter::new(&self.original_path).await;
        }

        // Open temp file for appending
        let file = fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&self.temp_path)
            .await?;

        // Rebuild hasher from existing data
        let mut hasher = Hasher::new();
        hasher.update(&existing_data);

        debug!(
            "Resumed atomic write: {} bytes already written",
            self.bytes_written
        );

        Ok(AtomicWriter {
            final_path: self.original_path,
            temp_path: self.temp_path,
            file: Some(file),
            hasher,
            bytes_written: self.bytes_written,
        })
    }

    /// Abandon the resumable write and clean up
    pub async fn abandon(self) -> Result<()> {
        fs::remove_file(&self.temp_path).await?;
        debug!("Abandoned resumable write: {}", self.temp_path.display());
        Ok(())
    }
}

/// Atomic directory operations
pub struct AtomicDirectory {
    final_path: PathBuf,
    temp_path: PathBuf,
}

impl AtomicDirectory {
    /// Create a new atomic directory operation
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let final_path = path.as_ref().to_path_buf();
        let temp_path = final_path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));

        // Create temporary directory
        fs::create_dir_all(&temp_path).await?;

        Ok(Self {
            final_path,
            temp_path,
        })
    }

    /// Get the temporary directory path for writing
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Commit the directory atomically
    pub async fn commit(self) -> Result<PathBuf> {
        // If target exists, remove it first
        if self.final_path.exists() {
            if self.final_path.is_dir() {
                fs::remove_dir_all(&self.final_path).await?;
            } else {
                fs::remove_file(&self.final_path).await?;
            }
        }

        // Atomic move
        fs::rename(&self.temp_path, &self.final_path).await?;

        debug!("Committed atomic directory: {}", self.final_path.display());
        Ok(self.final_path)
    }

    /// Abort the operation and clean up
    pub async fn abort(self) -> Result<()> {
        if self.temp_path.exists() {
            fs::remove_dir_all(&self.temp_path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_atomic_writer() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");

        // Write data atomically
        let mut writer = AtomicWriter::new(&test_file).await.unwrap();
        writer.write(b"Hello, ").await.unwrap();
        writer.write(b"World!").await.unwrap();

        // File shouldn't exist yet
        assert!(!test_file.exists());

        // Commit the write
        let result = writer.commit().await.unwrap();

        // Now file should exist
        assert!(test_file.exists());
        assert_eq!(result.bytes_written, 13);

        // Verify content
        let content = fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_atomic_writer_abort() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");

        let mut writer = AtomicWriter::new(&test_file).await.unwrap();
        writer.write(b"This should be discarded").await.unwrap();

        // Abort the write
        writer.abort().await.unwrap();

        // File shouldn't exist
        assert!(!test_file.exists());
    }

    #[tokio::test]
    async fn test_resumable_write() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");

        // Start a write but don't commit
        let mut writer = AtomicWriter::new(&test_file).await.unwrap();
        writer.write(b"Part 1").await.unwrap();
        writer.flush().await.unwrap();

        // Simulate crash - don't commit, just drop
        let temp_path = writer.temp_path.clone();
        drop(writer);

        // Check for resumable write
        let resumable = check_resumable_write(&test_file).await;
        assert!(resumable.is_some());

        let resumable = resumable.unwrap();
        assert_eq!(resumable.bytes_written, 6);

        // Resume writing
        let mut resumed_writer = resumable.resume().await.unwrap();
        resumed_writer.write(b" Part 2").await.unwrap();

        let result = resumed_writer.commit().await.unwrap();
        assert_eq!(result.bytes_written, 13);

        // Verify final content
        let content = fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "Part 1 Part 2");
    }
}
