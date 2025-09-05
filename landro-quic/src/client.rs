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

/// QUIC client for establishing secure connections to peers.
///
/// The client handles QUIC connection establishment with mTLS authentication,
/// device identity verification, and protocol handshake.
///
/// # Example
///
/// ```rust,no_run
/// use landro_quic::{QuicClient, QuicConfig};
/// use landro_crypto::{DeviceIdentity, CertificateVerifier};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let identity = Arc::new(DeviceIdentity::generate("my-device")?);
/// let verifier = Arc::new(CertificateVerifier::allow_any()); // For pairing/testing
/// let config = QuicConfig::default();
///
/// let client = QuicClient::new(identity, verifier, config).await?;
/// let connection = client.connect("192.168.1.2:9876").await?;
/// # Ok(())
/// # }
/// ```
pub struct QuicClient {
    endpoint: Endpoint,
    identity: Arc<DeviceIdentity>,
    #[allow(dead_code)]
    verifier: Arc<CertificateVerifier>,
    #[allow(dead_code)]
    config: QuicConfig,
}

impl QuicClient {
    /// Create a new QUIC client with the specified identity and configuration.
    ///
    /// # Arguments
    ///
    /// * `identity` - The device identity for authentication
    /// * `verifier` - Certificate verifier for peer validation
    /// * `config` - QUIC configuration parameters
    ///
    /// # Returns
    ///
    /// A configured QUIC client ready to establish connections
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

    /// Connect to a peer at the specified address.
    ///
    /// This method resolves the address and establishes a QUIC connection with
    /// automatic TLS handshake and device identity exchange.
    ///
    /// # Arguments
    ///
    /// * `addr` - The address to connect to (can be hostname:port or IP:port)
    ///
    /// # Returns
    ///
    /// An established connection ready for data transfer
    pub async fn connect(&self, addr: impl ToSocketAddrs) -> Result<Connection> {
        let addr = addr
            .to_socket_addrs()
            .map_err(QuicError::Io)?
            .next()
            .ok_or_else(|| QuicError::Protocol("Invalid address".to_string()))?;

        self.connect_addr(addr).await
    }

    /// Connect to a peer with a resolved socket address.
    ///
    /// # Arguments
    ///
    /// * `addr` - The socket address to connect to
    ///
    /// # Returns
    ///
    /// An established connection ready for data transfer
    pub async fn connect_addr(&self, addr: SocketAddr) -> Result<Connection> {
        info!("Connecting to {}", addr);

        // Initiate connection
        let connecting = self
            .endpoint
            .connect(addr, "localhost")
            .map_err(QuicError::Connect)?;

        // Apply timeout to connection establishment
        let quinn_conn = timeout(Duration::from_secs(10), connecting)
            .await
            .map_err(|_| QuicError::Timeout)?
            .map_err(QuicError::Connection)?;

        info!("Connected to {}", addr);

        let connection = Connection::new(quinn_conn);

        // Perform client-side handshake
        let device_id = self.identity.device_id();
        let device_name = self.identity.device_name();

        match connection
            .client_handshake(device_id.as_bytes(), device_name)
            .await
        {
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

    /// Connect to a peer with automatic retry on failure.
    ///
    /// This method attempts to establish a connection multiple times with
    /// exponential backoff between attempts.
    ///
    /// # Arguments
    ///
    /// * `addr` - The socket address to connect to
    /// * `max_attempts` - Maximum number of connection attempts
    /// * `retry_delay` - Delay between retry attempts
    ///
    /// # Returns
    ///
    /// An established connection ready for data transfer
    pub async fn connect_with_retry(
        &self,
        addr: SocketAddr,
        max_attempts: u32,
        retry_delay: Duration,
    ) -> Result<Connection> {
        let mut last_error = None;

        for attempt in 1..=max_attempts {
            debug!(
                "Connection attempt {} of {} to {}",
                attempt, max_attempts, addr
            );

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

        Err(last_error.unwrap_or_else(|| QuicError::Protocol("Connection failed".to_string())))
    }

    /// Gracefully shutdown the client and close all connections.
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
