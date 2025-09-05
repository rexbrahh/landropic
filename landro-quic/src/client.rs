use quinn::Endpoint;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info};

use landro_crypto::{CertificateVerifier, DeviceIdentity};

use crate::config::QuicConfig;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// QUIC client for connecting to peers
pub struct QuicClient {
    endpoint: Endpoint,
    identity: Arc<DeviceIdentity>,
    verifier: Arc<CertificateVerifier>,
    config: QuicConfig,
}

impl QuicClient {
    /// Create a new QUIC client
    pub async fn new(
        identity: Arc<DeviceIdentity>,
        verifier: Arc<CertificateVerifier>,
        config: QuicConfig,
    ) -> Result<Self> {
        // Build client configuration
        let client_config = config.build_client_config(&identity, verifier.clone())?;
        
        // Create client endpoint
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())?;
        endpoint.set_default_client_config(client_config);
        
        info!("QUIC client initialized");
        debug!("Device ID: {}", identity.device_id());
        
        Ok(Self {
            endpoint,
            identity,
            verifier,
            config,
        })
    }
    
    /// Connect to a peer
    pub async fn connect(&self, addr: impl ToSocketAddrs) -> Result<Connection> {
        let addr = addr.to_socket_addrs()
            .map_err(|e| QuicError::Io(e))?
            .next()
            .ok_or_else(|| QuicError::Protocol("Invalid address".to_string()))?;
        
        self.connect_addr(addr).await
    }
    
    /// Connect to a peer with socket address
    pub async fn connect_addr(&self, addr: SocketAddr) -> Result<Connection> {
        info!("Connecting to {}", addr);
        
        // Initiate connection
        let connecting = self.endpoint
            .connect(addr, "localhost")
            .map_err(|e| QuicError::Connect(e))?;
        
        // Apply timeout to connection establishment
        let quinn_conn = timeout(
            Duration::from_secs(10),
            connecting
        )
        .await
        .map_err(|_| QuicError::Timeout)?
        .map_err(|e| QuicError::Connection(e))?;
        
        info!("Connected to {}", addr);
        
        let connection = Connection::new(quinn_conn);
        
        // Perform handshake
        let device_id = self.identity.device_id();
        let device_name = self.identity.device_name();
        
        match connection.handshake(device_id.as_bytes(), device_name).await {
            Ok(()) => {
                info!("Handshake completed with {}", addr);
                Ok(connection)
            }
            Err(e) => {
                error!("Handshake failed with {}: {}", addr, e);
                connection.close(1, b"handshake failed");
                Err(e)
            }
        }
    }
    
    /// Connect with retry logic
    pub async fn connect_with_retry(
        &self, 
        addr: SocketAddr,
        max_attempts: u32,
        retry_delay: Duration,
    ) -> Result<Connection> {
        let mut last_error = None;
        
        for attempt in 1..=max_attempts {
            debug!("Connection attempt {} of {} to {}", attempt, max_attempts, addr);
            
            match self.connect_addr(addr).await {
                Ok(connection) => {
                    return Ok(connection);
                }
                Err(e) => {
                    error!("Connection attempt {} failed: {}", attempt, e);
                    last_error = Some(e);
                    
                    if attempt < max_attempts {
                        tokio::time::sleep(retry_delay).await;
                    }
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| 
            QuicError::Protocol("Connection failed".to_string())
        ))
    }
    
    /// Shutdown the client
    pub fn shutdown(&self) {
        self.endpoint.close(0u32.into(), b"client shutdown");
        info!("QUIC client shutdown");
    }
}

impl Drop for QuicClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_client_config_types() {
        // Install default crypto provider for rustls
        let _ = rustls::crypto::ring::default_provider().install_default();
        
        // Test that we can create basic config objects
        let config = QuicConfig::default();
        assert_eq!(config.bind_addr.port(), 9876);
        
        // This test just verifies the types compile correctly
        // Full integration tests with actual certificate generation
        // are handled in the landro-crypto integration tests
    }
}