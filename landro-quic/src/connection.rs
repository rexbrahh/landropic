use quinn::{Connection as QuinnConnection, RecvStream, SendStream};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, info};

use landro_proto::{
    Hello, VersionNegotiator, PROTOCOL_VERSION,
    validation::{self, limits, Validator},
};
use prost::Message;

use crate::errors::{QuicError, Result};

/// Stream type identifier
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamType {
    Control = 0,
    Data = 1,
}

/// Wrapper around Quinn connection with protocol handling
#[derive(Clone)]
pub struct Connection {
    inner: QuinnConnection,
    remote_device_id: Arc<Mutex<Option<Vec<u8>>>>,
    remote_device_name: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("remote_address", &self.inner.remote_address())
            .field("stable_id", &self.inner.stable_id())
            .finish()
    }
}

impl Connection {
    /// Create a new connection wrapper
    pub fn new(inner: QuinnConnection) -> Self {
        Self {
            inner,
            remote_device_id: Arc::new(Mutex::new(None)),
            remote_device_name: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the remote address
    pub fn remote_address(&self) -> std::net::SocketAddr {
        self.inner.remote_address()
    }

    /// Get the connection ID
    pub fn stable_id(&self) -> usize {
        self.inner.stable_id()
    }

    /// Check if connection is closed
    pub fn is_closed(&self) -> bool {
        self.inner.close_reason().is_some()
    }

    /// Close the connection
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.inner.close(code.into(), reason);
    }

    /// Open a bidirectional stream
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream)> {
        self.inner.open_bi().await.map_err(QuicError::Connection)
    }

    /// Open a unidirectional stream
    pub async fn open_uni(&self) -> Result<SendStream> {
        self.inner.open_uni().await.map_err(QuicError::Connection)
    }

    /// Accept a bidirectional stream
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream)> {
        self.inner.accept_bi().await.map_err(QuicError::Connection)
    }

    /// Accept a unidirectional stream
    pub async fn accept_uni(&self) -> Result<RecvStream> {
        self.inner.accept_uni().await.map_err(QuicError::Connection)
    }

    /// Send handshake message
    pub async fn send_hello(&self, device_id: &[u8], device_name: &str) -> Result<()> {
        // Validate inputs
        if device_id.len() != 32 {
            return Err(QuicError::Protocol(format!(
                "Invalid device ID length: expected 32 bytes, got {}",
                device_id.len()
            )));
        }
        
        Validator::validate_device_name(device_name)
            .map_err(|e| QuicError::Protocol(format!("Invalid device name: {}", e)))?;
        
        let hello = Hello {
            version: PROTOCOL_VERSION.to_string(),
            device_id: device_id.to_vec(),
            device_name: device_name.to_string(),
            capabilities: vec!["sync".to_string(), "transfer".to_string()],
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
        };

        let mut buf = Vec::new();
        hello
            .encode(&mut buf)
            .map_err(|e| QuicError::Protocol(format!("Failed to encode hello: {}", e)))?;
        
        // Check encoded size
        if buf.len() > limits::MAX_MESSAGE_SIZE {
            return Err(QuicError::Protocol(format!(
                "Hello message too large: {} bytes (max: {})",
                buf.len(),
                limits::MAX_MESSAGE_SIZE
            )));
        }

        // Open control stream (stream 0)
        let mut stream = self.open_uni().await?;

        // Write stream type marker
        stream
            .write_all(&[StreamType::Control as u8])
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to write stream type: {}", e)))?;

        // Write message length and data
        let len = buf.len() as u32;
        stream
            .write_all(&len.to_be_bytes())
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to write message length: {}", e)))?;
        stream
            .write_all(&buf)
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to write hello: {}", e)))?;

        stream
            .finish()
            .map_err(|e| QuicError::Stream(format!("Failed to finish stream: {}", e)))?;

        info!("Sent hello to {}", self.remote_address());
        Ok(())
    }

    /// Receive and process handshake message
    pub async fn receive_hello(&self) -> Result<()> {
        let mut stream = self.accept_uni().await?;

        // Read stream type
        let mut stream_type = [0u8; 1];
        stream
            .read_exact(&mut stream_type)
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to read stream type: {}", e)))?;

        if stream_type[0] != StreamType::Control as u8 {
            return Err(QuicError::Protocol(format!(
                "Expected control stream, got type {}",
                stream_type[0]
            )));
        }

        // Read message length
        let mut len_bytes = [0u8; 4];
        stream
            .read_exact(&mut len_bytes)
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to read message length: {}", e)))?;
        let len = u32::from_be_bytes(len_bytes) as usize;

        // Validate message size
        Validator::validate_message_size(len)
            .map_err(|e| QuicError::Protocol(format!("Invalid message size: {}", e)))?;

        // Read message data
        let mut buf = vec![0u8; len];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| QuicError::Stream(format!("Failed to read hello: {}", e)))?;

        // Parse hello message
        let hello = Hello::decode(&buf[..])
            .map_err(|e| QuicError::Protocol(format!("Failed to decode hello: {}", e)))?;

        // Validate hello message using comprehensive validation
        validation::validate_hello(&hello)
            .map_err(|e| QuicError::Protocol(format!("Invalid hello message: {}", e)))?;

        // Store remote device information
        *self.remote_device_id.lock().await = Some(hello.device_id.clone());
        *self.remote_device_name.lock().await = Some(hello.device_name.clone());

        info!(
            "Received hello from {} ({})",
            hello.device_name,
            hex::encode(&hello.device_id[..8])
        );

        Ok(())
    }

    /// Get remote device ID
    pub async fn remote_device_id(&self) -> Option<Vec<u8>> {
        self.remote_device_id.lock().await.clone()
    }

    /// Get remote device name
    pub async fn remote_device_name(&self) -> Option<String> {
        self.remote_device_name.lock().await.clone()
    }

    /// Perform client-side handshake (send hello, then receive response)
    pub async fn client_handshake(&self, device_id: &[u8], device_name: &str) -> Result<()> {
        self.client_handshake_with_timeout(device_id, device_name, Duration::from_secs(10))
            .await
    }

    /// Perform client-side handshake with custom timeout
    pub async fn client_handshake_with_timeout(
        &self,
        device_id: &[u8],
        device_name: &str,
        handshake_timeout: Duration,
    ) -> Result<()> {
        let timeout_secs = handshake_timeout.as_secs();
        let handshake_future = async {
            // Client: Send our hello first
            self.send_hello(device_id, device_name)
                .await
                .map_err(|e| QuicError::handshake_failed(format!("Failed to send hello: {}", e)))?;

            // Client: Receive server's hello response
            self.receive_hello().await.map_err(|e| {
                QuicError::handshake_failed(format!("Failed to receive server hello: {}", e))
            })?;

            debug!("Client handshake completed with {}", self.remote_address());
            Ok(())
        };

        timeout(handshake_timeout, handshake_future)
            .await
            .map_err(|_| QuicError::HandshakeTimeout { timeout_secs })?
    }

    /// Perform server-side handshake (receive hello, then send response)
    pub async fn server_handshake(&self, device_id: &[u8], device_name: &str) -> Result<()> {
        self.server_handshake_with_timeout(device_id, device_name, Duration::from_secs(10))
            .await
    }

    /// Perform server-side handshake with custom timeout
    pub async fn server_handshake_with_timeout(
        &self,
        device_id: &[u8],
        device_name: &str,
        handshake_timeout: Duration,
    ) -> Result<()> {
        let timeout_secs = handshake_timeout.as_secs();
        let handshake_future = async {
            // Server: Receive client's hello first
            self.receive_hello().await.map_err(|e| {
                QuicError::handshake_failed(format!("Failed to receive client hello: {}", e))
            })?;

            // Server: Send our hello response
            self.send_hello(device_id, device_name).await.map_err(|e| {
                QuicError::handshake_failed(format!("Failed to send hello response: {}", e))
            })?;

            debug!("Server handshake completed with {}", self.remote_address());
            Ok(())
        };

        timeout(handshake_timeout, handshake_future)
            .await
            .map_err(|_| QuicError::HandshakeTimeout { timeout_secs })?
    }

    /// Perform mutual handshake (deprecated - use client_handshake or server_handshake)
    pub async fn handshake(&self, device_id: &[u8], device_name: &str) -> Result<()> {
        // Default to client behavior for backward compatibility
        self.client_handshake(device_id, device_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_type() {
        assert_eq!(StreamType::Control as u8, 0);
        assert_eq!(StreamType::Data as u8, 1);
    }
}
