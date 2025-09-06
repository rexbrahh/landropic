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
            idle_timeout: Duration::from_secs(60), // Longer timeout for large transfers
            keep_alive_interval: Duration::from_secs(15), // Less aggressive keepalive
            max_concurrent_bidi_streams: 256,      // More streams for parallelism
            max_concurrent_uni_streams: 512,       // More unidirectional streams
            stream_receive_window: 16 * 1024 * 1024, // 16MB per stream
            receive_window: 256 * 1024 * 1024,     // 256MB connection window
            max_udp_payload_size: 1472,            // Optimal for Ethernet MTU (1500 - headers)
            initial_rtt_us: 200,                   // 200μs initial RTT for LAN
            max_ack_delay_ms: 5,                   // 5ms max ACK delay for LAN
            enable_datagrams: true,                // Enable for small control messages
            congestion_control: "cubic".to_string(), // CUBIC for LAN (can switch to BBR)
            send_buffer_size: 8 * 1024 * 1024,     // 8MB send buffer
            recv_buffer_size: 8 * 1024 * 1024,     // 8MB receive buffer
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
        // max_ack_delay is not available in current quinn version
        // transport.max_ack_delay(Duration::from_millis(self.max_ack_delay_ms));

        // Datagram support for small control messages
        if self.enable_datagrams {
            transport.datagram_receive_buffer_size(Some(65536)); // 64KB for datagrams
            transport.datagram_send_buffer_size(65536);
        }

        // Set congestion control algorithm
        match self.congestion_control.as_str() {
            "bbr" => {
                // BBR is good for networks with bufferbloat
                let mut bbr = congestion::BbrConfig::default();
                bbr.initial_window(100); // Start with larger window for LAN
                transport.congestion_controller_factory(Arc::new(bbr));
            }
            "cubic" | _ => {
                // CUBIC is default and works well for LAN
                let mut cubic = congestion::CubicConfig::default();
                cubic.initial_window(100); // Larger initial window for LAN
                transport.congestion_controller_factory(Arc::new(cubic));
            }
        }

        // Additional performance tuning
        transport.mtu_discovery_config(Some(Default::default())); // Enable MTU discovery

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
