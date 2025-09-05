use quinn::{ClientConfig, ServerConfig, TransportConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use landro_crypto::{CertificateGenerator, CertificateVerifier, DeviceIdentity};
use landro_crypto::certificate::TlsConfig;

use crate::errors::{QuicError, Result};

/// QUIC configuration builder
pub struct QuicConfig {
    /// Bind address for server
    pub bind_addr: SocketAddr,
    
    /// Maximum idle timeout
    pub idle_timeout: Duration,
    
    /// Keep-alive interval
    pub keep_alive_interval: Duration,
    
    /// Maximum concurrent bidirectional streams
    pub max_concurrent_bidi_streams: u64,
    
    /// Maximum concurrent unidirectional streams  
    pub max_concurrent_uni_streams: u64,
    
    /// Stream receive window
    pub stream_receive_window: u64,
    
    /// Connection receive window
    pub receive_window: u64,
    
    /// Maximum UDP payload size
    pub max_udp_payload_size: u16,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            idle_timeout: Duration::from_secs(30),
            keep_alive_interval: Duration::from_secs(10),
            max_concurrent_bidi_streams: 100,
            max_concurrent_uni_streams: 100,
            stream_receive_window: 1024 * 1024, // 1MB
            receive_window: 10 * 1024 * 1024, // 10MB
            max_udp_payload_size: 1350, // Conservative for IPv6
        }
    }
}

impl QuicConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set bind address
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }
    
    /// Set idle timeout
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
    
    /// Build transport configuration
    fn build_transport_config(&self) -> TransportConfig {
        let mut transport = TransportConfig::default();
        
        transport.max_idle_timeout(Some(self.idle_timeout.try_into().unwrap()));
        transport.keep_alive_interval(Some(self.keep_alive_interval));
        transport.max_concurrent_bidi_streams(self.max_concurrent_bidi_streams.try_into().unwrap());
        transport.max_concurrent_uni_streams(self.max_concurrent_uni_streams.try_into().unwrap());
        transport.stream_receive_window(self.stream_receive_window.try_into().unwrap());
        transport.receive_window(self.receive_window.try_into().unwrap());
        // Note: max_udp_payload_size is no longer configurable in recent Quinn versions
        // transport.max_udp_payload_size(self.max_udp_payload_size);
        
        // Enable BBR congestion control if available
        // transport.congestion_controller_factory(Box::new(quinn::congestion::BbrConfig::default()));
        
        transport
    }
    
    /// Build server configuration
    pub fn build_server_config(
        &self,
        identity: &DeviceIdentity,
        verifier: Arc<CertificateVerifier>,
    ) -> Result<ServerConfig> {
        // Generate certificate for this device
        let (cert_chain, private_key) = 
            CertificateGenerator::generate_device_certificate(identity)?;
        
        // Create TLS configuration
        let tls_config = TlsConfig::server_config(cert_chain, private_key, verifier)?;
        
        // Build Quinn server configuration
        let crypto_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .map_err(|e| QuicError::TlsConfig(format!("Failed to create QUIC server config: {}", e)))?;
        let mut config = ServerConfig::with_crypto(Arc::new(crypto_config));
        config.transport_config(Arc::new(self.build_transport_config()));
        
        Ok(config)
    }
    
    /// Build client configuration
    pub fn build_client_config(
        &self,
        identity: &DeviceIdentity,
        verifier: Arc<CertificateVerifier>,
    ) -> Result<ClientConfig> {
        // Generate certificate for this device
        let (cert_chain, private_key) = 
            CertificateGenerator::generate_device_certificate(identity)?;
        
        // Create TLS configuration
        let tls_config = TlsConfig::client_config(cert_chain, private_key, verifier)?;
        
        // Build Quinn client configuration
        let crypto_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| QuicError::TlsConfig(format!("Failed to create QUIC client config: {}", e)))?;
        let mut config = ClientConfig::new(Arc::new(crypto_config));
        config.transport_config(Arc::new(self.build_transport_config()));
        
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_config() {
        let config = QuicConfig::default();
        assert_eq!(config.bind_addr.port(), 9876);
        assert_eq!(config.idle_timeout, Duration::from_secs(30));
    }
    
    #[test]
    fn test_config_builder() {
        let addr: SocketAddr = "127.0.0.1:8888".parse().unwrap();
        let config = QuicConfig::new()
            .bind_addr(addr)
            .idle_timeout(Duration::from_secs(60));
        
        assert_eq!(config.bind_addr, addr);
        assert_eq!(config.idle_timeout, Duration::from_secs(60));
    }
}