use quinn::Endpoint;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use landro_crypto::{CertificateVerifier, DeviceIdentity};

use crate::config::QuicConfig;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// QUIC server for accepting peer connections
pub struct QuicServer {
    endpoint: Option<Endpoint>,
    identity: Arc<DeviceIdentity>,
    verifier: Arc<CertificateVerifier>,
    config: QuicConfig,
    connections: Arc<RwLock<Vec<Connection>>>,
}

impl QuicServer {
    /// Create a new QUIC server
    pub fn new(
        identity: Arc<DeviceIdentity>,
        verifier: Arc<CertificateVerifier>,
        config: QuicConfig,
    ) -> Self {
        Self {
            endpoint: None,
            identity,
            verifier,
            config,
            connections: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Start the server
    pub async fn start(&mut self) -> Result<()> {
        if self.endpoint.is_some() {
            return Err(QuicError::ServerAlreadyRunning);
        }

        // Build server configuration
        let server_config = self
            .config
            .build_server_config(&self.identity, self.verifier.clone())?;

        // Create endpoint
        let endpoint = Endpoint::server(server_config, self.config.bind_addr)?;

        info!("QUIC server listening on {}", self.config.bind_addr);
        info!("Device ID: {}", self.identity.device_id());

        self.endpoint = Some(endpoint);
        Ok(())
    }

    /// Stop the server
    pub fn stop(&mut self) {
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.close(0u32.into(), b"server shutdown");
            info!("QUIC server stopped");
        }
    }

    /// Accept incoming connections
    pub async fn accept(&self) -> Result<Connection> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| QuicError::Protocol("Server not started".to_string()))?;

        // Accept connection
        let connecting = endpoint
            .accept()
            .await
            .ok_or_else(|| QuicError::Protocol("Endpoint closed".to_string()))?;

        let remote_addr = connecting.remote_address();
        info!("Accepting connection from {}", remote_addr);

        // Complete connection establishment
        let quinn_conn = connecting.await.map_err(QuicError::Connection)?;

        let connection = Connection::new(quinn_conn);

        // Perform handshake
        let device_id = self.identity.device_id();
        let device_name = self.identity.device_name();

        match connection
            .handshake(device_id.as_bytes(), device_name)
            .await
        {
            Ok(()) => {
                info!("Connection established with {}", remote_addr);

                // Store connection
                self.connections.write().await.push(connection.clone());

                Ok(connection)
            }
            Err(e) => {
                error!("Handshake failed with {}: {}", remote_addr, e);
                connection.close(1, b"handshake failed");
                Err(e)
            }
        }
    }

    /// Run the server accept loop
    pub async fn run(&self) -> Result<()> {
        loop {
            match self.accept().await {
                Ok(connection) => {
                    // Handle connection in background
                    let connections = self.connections.clone();
                    let remote_addr = connection.remote_address();

                    tokio::spawn(async move {
                        // Connection handler would go here
                        info!("Handling connection from {}", remote_addr);

                        // For now, just keep connection alive
                        while !connection.is_closed() {
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }

                        info!("Connection closed from {}", remote_addr);

                        // Remove from connections list
                        let mut conns = connections.write().await;
                        conns.retain(|c| c.stable_id() != connection.stable_id());
                    });
                }
                Err(e) => {
                    if matches!(e, QuicError::Protocol(_)) {
                        // Endpoint closed, exit loop
                        break;
                    }
                    warn!("Failed to accept connection: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Get list of active connections
    pub async fn connections(&self) -> Vec<Connection> {
        self.connections.read().await.clone()
    }

    /// Get bind address
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.endpoint.as_ref().and_then(|e| e.local_addr().ok())
    }
}

impl Drop for QuicServer {
    fn drop(&mut self) {
        self.stop();
    }
}
