//! Zero-copy I/O optimizations for high-performance file transfers

use bytes::{Bytes, BytesMut};
use quinn::SendStream;
use std::fs::File;
use std::io::{self, SeekFrom};
use std::os::unix::fs::FileExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error};

use crate::errors::{QuicError, Result};

/// Zero-copy file reader using memory-mapped I/O and vectored reads
pub struct ZeroCopyFileReader {
    file: Arc<TokioFile>,
    chunk_size: usize,
    use_direct_io: bool,
}

impl ZeroCopyFileReader {
    /// Create a new zero-copy file reader
    pub async fn new(path: impl AsRef<Path>, chunk_size: usize) -> io::Result<Self> {
        let file = TokioFile::open(path).await?;

        Ok(Self {
            file: Arc::new(file),
            chunk_size,
            use_direct_io: Self::check_direct_io_support(),
        })
    }

    /// Check if the platform supports direct I/O
    fn check_direct_io_support() -> bool {
        // Direct I/O is platform-specific
        // On Linux, we could use O_DIRECT flag
        // For now, we'll use standard async I/O
        false
    }

    /// Read a chunk at the specified offset using zero-copy techniques
    pub async fn read_chunk(&self, offset: u64, size: usize) -> Result<Bytes> {
        // Use pread for positioned read without seeking
        let file = self.file.clone();
        let mut temp_buffer = vec![0u8; size];
        let bytes_read = tokio::task::spawn_blocking(move || {
            let std_file = unsafe {
                // Safety: We're just borrowing the file descriptor for a read operation
                std::fs::File::from_raw_fd(file.as_raw_fd())
            };

            let result = std_file.read_at(&mut temp_buffer[..], offset);

            // Prevent the file from being closed when std_file is dropped
            std::mem::forget(std_file);

            result.map(|n| (n, temp_buffer))
        })
        .await
        .map_err(|e| QuicError::Io(e.into()))?
        .map_err(|e| QuicError::Io(e))?;

        let mut buffer = BytesMut::with_capacity(bytes_read.0);
        buffer.extend_from_slice(&bytes_read.1[..bytes_read.0]);
        Ok(buffer.freeze())
    }

    /// Read multiple chunks in a single vectored read operation
    pub async fn read_chunks_vectored(&self, offsets: &[(u64, usize)]) -> Result<Vec<Bytes>> {
        let mut results = Vec::with_capacity(offsets.len());

        // For now, read sequentially - could be optimized with io_uring on Linux
        for &(offset, size) in offsets {
            results.push(self.read_chunk(offset, size).await?);
        }

        Ok(results)
    }
}

/// Zero-copy stream writer optimized for QUIC
pub struct ZeroCopyStreamWriter {
    chunk_size: usize,
    buffer_pool: Arc<BufferPool>,
}

impl ZeroCopyStreamWriter {
    /// Create a new zero-copy stream writer
    pub fn new(chunk_size: usize) -> Self {
        Self {
            chunk_size,
            buffer_pool: Arc::new(BufferPool::new(32, chunk_size)),
        }
    }

    /// Write data to a QUIC stream using zero-copy techniques
    pub async fn write_to_stream(&self, stream: &mut SendStream, data: &[u8]) -> Result<()> {
        // For large data, use chunked writing to avoid memory pressure
        if data.len() > self.chunk_size {
            for chunk in data.chunks(self.chunk_size) {
                stream
                    .write_all(chunk)
                    .await
                    .map_err(|e| QuicError::Stream(format!("Write failed: {}", e)))?;
            }
        } else {
            stream
                .write_all(data)
                .await
                .map_err(|e| QuicError::Stream(format!("Write failed: {}", e)))?;
        }

        Ok(())
    }

    /// Write file directly to stream using sendfile-like optimization
    pub async fn sendfile_to_stream(
        &self,
        stream: &mut SendStream,
        file: &TokioFile,
        offset: u64,
        size: usize,
    ) -> Result<usize> {
        // Read from file and write to stream in chunks
        let mut total_written = 0;
        let mut current_offset = offset;

        while total_written < size {
            let chunk_size = (size - total_written).min(self.chunk_size);
            let mut buffer = self.buffer_pool.get().await;
            buffer.resize(chunk_size, 0);

            // Read chunk from file
            let bytes_read = {
                let file = file.try_clone().await.map_err(|e| QuicError::Io(e))?;

                // Create a temporary buffer for the blocking operation
                let mut temp_buffer = vec![0u8; chunk_size];
                let bytes = tokio::task::spawn_blocking(move || {
                    use std::os::unix::fs::FileExt;
                    let std_file = unsafe { std::fs::File::from_raw_fd(file.as_raw_fd()) };

                    let result = std_file.read_at(&mut temp_buffer[..], current_offset);
                    std::mem::forget(std_file);
                    result.map(|n| (n, temp_buffer))
                })
                .await
                .map_err(|e| QuicError::Io(e.into()))?
                .map_err(|e| QuicError::Io(e))?;

                // Copy data back to our buffer
                buffer[..bytes.0].copy_from_slice(&bytes.1[..bytes.0]);
                bytes.0
            };

            if bytes_read == 0 {
                break;
            }

            // Write to stream
            stream
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(|e| QuicError::Stream(format!("Stream write failed: {}", e)))?;

            total_written += bytes_read;
            current_offset += bytes_read as u64;

            // Return buffer to pool
            self.buffer_pool.return_buffer(buffer).await;
        }

        Ok(total_written)
    }
}

/// Memory-mapped file reader for extremely fast access
pub struct MmapFileReader {
    mmap: memmap2::Mmap,
    file_size: u64,
}

impl MmapFileReader {
    /// Create a new memory-mapped file reader
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();

        // Safety: We're only reading from the mmap
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };

        Ok(Self { mmap, file_size })
    }

    /// Get a slice of the file at the specified range
    pub fn get_slice(&self, offset: u64, size: usize) -> Option<&[u8]> {
        let offset = offset as usize;
        let end = offset + size;

        if end <= self.mmap.len() {
            Some(&self.mmap[offset..end])
        } else {
            None
        }
    }

    /// Get the entire file as a slice
    pub fn as_slice(&self) -> &[u8] {
        &self.mmap[..]
    }

    /// Get file size
    pub fn size(&self) -> u64 {
        self.file_size
    }
}

/// Buffer pool for reducing allocation overhead
struct BufferPool {
    buffers: tokio::sync::Mutex<Vec<BytesMut>>,
    buffer_size: usize,
    max_buffers: usize,
}

impl BufferPool {
    fn new(max_buffers: usize, buffer_size: usize) -> Self {
        let mut buffers = Vec::with_capacity(max_buffers);

        // Pre-allocate some buffers
        for _ in 0..max_buffers / 4 {
            buffers.push(BytesMut::with_capacity(buffer_size));
        }

        Self {
            buffers: tokio::sync::Mutex::new(buffers),
            buffer_size,
            max_buffers,
        }
    }

    async fn get(&self) -> BytesMut {
        let mut buffers = self.buffers.lock().await;

        if let Some(mut buffer) = buffers.pop() {
            buffer.clear();
            buffer
        } else {
            BytesMut::with_capacity(self.buffer_size)
        }
    }

    async fn return_buffer(&self, mut buffer: BytesMut) {
        buffer.clear();

        let mut buffers = self.buffers.lock().await;
        if buffers.len() < self.max_buffers {
            buffers.push(buffer);
        }
    }
}

/// Vectored I/O writer for efficient batched writes
pub struct VectoredWriter {
    buffers: Vec<Bytes>,
    total_size: usize,
    max_buffers: usize,
}

impl VectoredWriter {
    /// Create a new vectored writer
    pub fn new(max_buffers: usize) -> Self {
        Self {
            buffers: Vec::with_capacity(max_buffers),
            total_size: 0,
            max_buffers,
        }
    }

    /// Add a buffer to the vectored write batch
    pub fn add_buffer(&mut self, buffer: Bytes) -> bool {
        if self.buffers.len() >= self.max_buffers {
            return false;
        }

        self.total_size += buffer.len();
        self.buffers.push(buffer);
        true
    }

    /// Perform vectored write to a stream
    pub async fn write_vectored(&mut self, stream: &mut SendStream) -> Result<usize> {
        let mut written = 0;

        // Quinn doesn't directly support vectored writes, so we write sequentially
        // In a real implementation, we could use lower-level APIs or io_uring
        for buffer in &self.buffers {
            stream
                .write_all(buffer)
                .await
                .map_err(|e| QuicError::Stream(format!("Vectored write failed: {}", e)))?;
            written += buffer.len();
        }

        self.buffers.clear();
        self.total_size = 0;

        Ok(written)
    }

    /// Get the total size of buffered data
    pub fn buffered_size(&self) -> usize {
        self.total_size
    }

    /// Check if the writer is empty
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

// Platform-specific optimizations
#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::io::RawFd;

    /// Use sendfile system call for zero-copy file transfer (Linux)
    pub fn sendfile(
        out_fd: RawFd,
        in_fd: RawFd,
        offset: Option<&mut i64>,
        count: usize,
    ) -> io::Result<usize> {
        // This would use the actual sendfile syscall
        // For now, this is a placeholder
        unimplemented!("sendfile not yet implemented")
    }

    /// Use splice for zero-copy pipe transfer (Linux)
    pub fn splice(fd_in: RawFd, fd_out: RawFd, len: usize) -> io::Result<usize> {
        // This would use the actual splice syscall
        // For now, this is a placeholder
        unimplemented!("splice not yet implemented")
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::os::unix::io::RawFd;

    /// Use sendfile system call for zero-copy file transfer (macOS)
    pub fn sendfile(_fd: RawFd, _s: RawFd, _offset: i64, _len: Option<i64>) -> std::io::Result<usize> {
        // This would use the actual sendfile syscall on macOS
        // For now, this is a placeholder
        unimplemented!("sendfile not yet implemented for macOS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_zero_copy_file_reader() {
        // Create a test file
        let mut temp_file = NamedTempFile::new().unwrap();
        let test_data = b"Hello, zero-copy world!";
        temp_file.write_all(test_data).unwrap();
        temp_file.flush().unwrap();

        // Test reading
        let reader = ZeroCopyFileReader::new(temp_file.path(), 1024)
            .await
            .unwrap();
        let chunk = reader.read_chunk(0, test_data.len()).await.unwrap();

        assert_eq!(&chunk[..], test_data);
    }

    #[test]
    fn test_mmap_file_reader() {
        // Create a test file
        let mut temp_file = NamedTempFile::new().unwrap();
        let test_data = b"Memory-mapped file test data";
        temp_file.write_all(test_data).unwrap();
        temp_file.flush().unwrap();

        // Test memory-mapped reading
        let reader = MmapFileReader::new(temp_file.path()).unwrap();
        assert_eq!(reader.size(), test_data.len() as u64);

        let slice = reader.get_slice(0, test_data.len()).unwrap();
        assert_eq!(slice, test_data);

        let slice = reader.get_slice(7, 6).unwrap();
        assert_eq!(slice, b"mapped");
    }

    #[test]
    fn test_vectored_writer() {
        let mut writer = VectoredWriter::new(10);

        assert!(writer.is_empty());

        writer.add_buffer(Bytes::from_static(b"Hello"));
        writer.add_buffer(Bytes::from_static(b" "));
        writer.add_buffer(Bytes::from_static(b"World"));

        assert_eq!(writer.buffered_size(), 11);
        assert!(!writer.is_empty());
    }
}
