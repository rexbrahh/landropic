use quinn::{congestion, ClientConfig, ServerConfig, TransportConfig, VarInt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use landro_crypto::certificate::TlsConfig;
use landro_crypto::{CertificateGenerator, CertificateVerifier, DeviceIdentity};

use crate::errors::{QuicError, Result};

/// QUIC configuration builder optimized for LAN file transfers
#[derive(Clone)]
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

    /// Stream receive window (per-stream flow control)
    pub stream_receive_window: u64,

    /// Connection receive window (connection-level flow control)
    pub receive_window: u64,

    /// Maximum UDP payload size
    pub max_udp_payload_size: u16,

    /// Initial round-trip time estimate for LAN (microseconds)
    pub initial_rtt_us: u64,

    /// Maximum ACK delay for LAN (milliseconds)
    pub max_ack_delay_ms: u64,

    /// Enable datagram support for small messages
    pub enable_datagrams: bool,

    /// Congestion control algorithm ("cubic" or "bbr")
    pub congestion_control: String,

    /// Send buffer size per stream (bytes)
    pub send_buffer_size: usize,

    /// Receive buffer size per stream (bytes)
    pub recv_buffer_size: usize,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self::lan_optimized()
    }
}

impl QuicConfig {
    /// Create configuration optimized for LAN transfers
    pub fn lan_optimized() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            idle_timeout: Duration::from_secs(180), // 3 minutes for large file syncs
            keep_alive_interval: Duration::from_secs(30), // Balanced for file sync
            max_concurrent_bidi_streams: 1024,     // Increased for better parallelism
            max_concurrent_uni_streams: 2048,      // More streams for file metadata
            stream_receive_window: 128 * 1024 * 1024, // 128MB per stream for large chunks
            receive_window: 2 * 1024 * 1024 * 1024, // 2GB connection window for bulk transfers
            max_udp_payload_size: 1472,            // Optimal for Ethernet MTU (1500 - headers)
            initial_rtt_us: 50,                    // 50μs for fast LAN
            max_ack_delay_ms: 1,                   // 1ms for lower latency
            enable_datagrams: true,                // Enable for small control messages
            congestion_control: "bbr".to_string(), // BBR for better throughput even on LAN
            send_buffer_size: 64 * 1024 * 1024,    // 64MB send buffer for bulk data
            recv_buffer_size: 64 * 1024 * 1024,    // 64MB receive buffer for bulk data
        }
    }

    /// Create configuration optimized for file sync workloads
    pub fn file_sync_optimized() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            idle_timeout: Duration::from_secs(600), // 10 minutes for massive transfers
            keep_alive_interval: Duration::from_secs(45), // Balanced for responsiveness
            max_concurrent_bidi_streams: 2048,     // Maximum parallelism for file chunks
            max_concurrent_uni_streams: 4096,      // Many control/metadata streams
            stream_receive_window: 256 * 1024 * 1024, // 256MB for large file chunks
            receive_window: 4 * 1024 * 1024 * 1024, // 4GB for massive parallel transfers
            max_udp_payload_size: 1472,            // Optimal for Ethernet MTU
            initial_rtt_us: 25,                    // Ultra-aggressive for LAN
            max_ack_delay_ms: 1,                   // Minimal ACK delay
            enable_datagrams: true,                // For quick metadata exchange
            congestion_control: "bbr".to_string(), // BBR for better throughput
            send_buffer_size: 128 * 1024 * 1024,   // 128MB for chunk batching
            recv_buffer_size: 128 * 1024 * 1024,   // 128MB for chunk reception
        }
    }

    /// Create configuration for WAN/Internet transfers
    pub fn wan_optimized() -> Self {
        Self {
            bind_addr: "[::]:9876".parse().unwrap(),
            idle_timeout: Duration::from_secs(30),
            keep_alive_interval: Duration::from_secs(10),
            max_concurrent_bidi_streams: 100,
            max_concurrent_uni_streams: 100,
            stream_receive_window: 4 * 1024 * 1024, // 4MB per stream
            receive_window: 32 * 1024 * 1024,       // 32MB connection window
            max_udp_payload_size: 1200,             // Conservative for Internet
            initial_rtt_us: 50_000,                 // 50ms initial RTT
            max_ack_delay_ms: 25,                   // Standard 25ms
            enable_datagrams: false,                // Disable for reliability
            congestion_control: "bbr".to_string(),  // BBR for WAN
            send_buffer_size: 2 * 1024 * 1024,      // 2MB send buffer
            recv_buffer_size: 2 * 1024 * 1024,      // 2MB receive buffer
        }
    }

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

    /// Set congestion control algorithm
    pub fn congestion_control(mut self, algo: &str) -> Self {
        self.congestion_control = algo.to_string();
        self
    }

    /// Set stream receive window
    pub fn stream_receive_window(mut self, size: u64) -> Self {
        self.stream_receive_window = size;
        self
    }

    /// Set connection receive window
    pub fn receive_window(mut self, size: u64) -> Self {
        self.receive_window = size;
        self
    }

    /// Build transport configuration optimized for performance
    fn build_transport_config(&self) -> TransportConfig {
        let mut transport = TransportConfig::default();

        // Basic timeout and stream settings
        transport.max_idle_timeout(Some(self.idle_timeout.try_into().unwrap()));
        transport.keep_alive_interval(Some(self.keep_alive_interval));
        transport.max_concurrent_bidi_streams(
            VarInt::from_u64(self.max_concurrent_bidi_streams).unwrap(),
        );
        transport
            .max_concurrent_uni_streams(VarInt::from_u64(self.max_concurrent_uni_streams).unwrap());

        // Flow control windows - critical for throughput
        transport.stream_receive_window(VarInt::from_u64(self.stream_receive_window).unwrap());
        transport.receive_window(VarInt::from_u64(self.receive_window).unwrap());
        transport.send_window(self.receive_window); // Match send window to receive window

        // LAN optimizations
        transport.initial_rtt(Duration::from_micros(self.initial_rtt_us));
        
        // File sync specific optimizations
        transport.max_concurrent_bidi_streams(VarInt::from_u64(self.max_concurrent_bidi_streams).unwrap());
        
        // Datagram support for small control messages
        if self.enable_datagrams {
            transport.datagram_receive_buffer_size(Some(262144)); // 256KB for metadata
            transport.datagram_send_buffer_size(262144);
        }

        // Set congestion control algorithm with file sync optimizations
        match self.congestion_control.as_str() {
            "bbr" => {
                // BBR with aggressive settings for file sync
                let mut bbr = congestion::BbrConfig::default();
                bbr.initial_window(500); // Much larger window for bulk transfers
                transport.congestion_controller_factory(Arc::new(bbr));
            }
            "cubic" | _ => {
                // CUBIC with file sync optimizations
                let mut cubic = congestion::CubicConfig::default();
                cubic.initial_window(300); // Larger initial window for file chunks
                transport.congestion_controller_factory(Arc::new(cubic));
            }
        }

        // Additional performance tuning for file sync
        transport.mtu_discovery_config(Some(Default::default())); // Enable MTU discovery
        
        // Optimize for bulk data transfer patterns
        transport.stream_receive_window(VarInt::from_u64(self.stream_receive_window).unwrap());
        
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
        let crypto_config =
            quinn::crypto::rustls::QuicServerConfig::try_from(tls_config).map_err(|e| {
                QuicError::TlsConfig(format!("Failed to create QUIC server config: {}", e))
            })?;
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
        let crypto_config =
            quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).map_err(|e| {
                QuicError::TlsConfig(format!("Failed to create QUIC client config: {}", e))
            })?;
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
