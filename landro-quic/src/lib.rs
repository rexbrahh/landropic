//! # Landropic QUIC Transport Layer
//!
//! This crate provides the QUIC transport layer for Landropic, implementing secure,
//! encrypted communication between devices using QUIC with mutual TLS (mTLS) authentication.
//!
//! ## Overview
//!
//! The QUIC transport layer serves as the foundation for all network communication
//! in Landropic, providing:
//!
//! - **Secure connections**: QUIC with TLS 1.3 and certificate-based device authentication
//! - **Performance**: Low latency connection establishment and efficient data transfer
//! - **Reliability**: Built-in congestion control and automatic stream multiplexing
//! - **Device identity**: Integration with Landropic's device identity system
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐    ┌─────────────┐
//! │   Client    │    │   Server    │
//! │             │    │             │
//! │ QuicClient  │◄──►│ QuicServer  │
//! │             │    │             │
//! └─────┬───────┘    └─────┬───────┘
//!       │                  │
//!       └──────────────────┘
//!              │
//!       ┌─────────────┐
//!       │ Connection  │ ◄─── Wraps Quinn connection
//!       │             │
//!       │ - Control   │ ◄─── Protocol handshake
//!       │ - Data      │ ◄─── File transfer streams
//!       └─────────────┘
//! ```
//!
//! ## Core Components
//!
//! - [`QuicClient`] - Initiates outbound connections to peers
//! - [`QuicServer`] - Accepts inbound connections from peers  
//! - [`Connection`] - Represents an established connection with stream management
//! - [`QuicConfig`] - Configuration builder for QUIC transport parameters
//! - [`QuicError`] - Comprehensive error handling for transport operations
//!
//! ## Usage Examples
//!
//! ### Setting up a QUIC server
//!
//! ```rust,no_run
//! use landro_quic::{QuicServer, QuicConfig};
//! use landro_crypto::{DeviceIdentity, CertificateVerifier};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let identity = Arc::new(DeviceIdentity::generate("server-device")?);
//! let verifier = Arc::new(CertificateVerifier::for_pairing()); // For testing
//! let config = QuicConfig::default();
//!
//! let mut server = QuicServer::new(identity, verifier, config);
//! server.start().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Connecting to a peer
//!
//! ```rust,no_run
//! use landro_quic::{QuicClient, QuicConfig};
//! use landro_crypto::{DeviceIdentity, CertificateVerifier};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let identity = Arc::new(DeviceIdentity::generate("client-device")?);
//! let verifier = Arc::new(CertificateVerifier::for_pairing());
//! let config = QuicConfig::default();
//!
//! let client = QuicClient::new(identity, verifier, config).await?;
//! let connection = client.connect("192.168.1.2:9876").await?;
//!
//! // Use connection for data transfer
//! let (mut send, mut recv) = connection.open_bi().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Security Model
//!
//! The QUIC transport implements Landropic's security model:
//!
//! 1. **Device Identity**: Each device has a unique Ed25519 identity key
//! 2. **Certificate Generation**: Automatic X.509 certificate generation from device identity
//! 3. **Mutual TLS**: Both client and server authenticate each other's certificates
//! 4. **Certificate Pinning**: Devices verify expected certificate fingerprints
//! 5. **Forward Secrecy**: TLS 1.3 ephemeral key exchange ensures forward secrecy
//!
//! ## Performance Characteristics
//!
//! - **Connection establishment**: ~1 RTT with QUIC 0-RTT when possible
//! - **Stream multiplexing**: Multiple streams per connection without head-of-line blocking  
//! - **Congestion control**: BBR congestion control for optimal throughput
//! - **Packet loss recovery**: Built-in QUIC packet recovery mechanisms
//!
//! ## Error Handling
//!
//! All operations return [`Result<T, QuicError>`](QuicError) for comprehensive error handling:
//!
//! ```rust,no_run
//! use landro_quic::{QuicClient, QuicError};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let client: QuicClient = todo!(); // Assume client is created
//! match client.connect("invalid-address").await {
//!     Ok(connection) => {
//!         // Use connection
//!     }
//!     Err(QuicError::Io(e)) => {
//!         eprintln!("Network error: {}", e);
//!     }
//!     Err(QuicError::Connect(e)) => {
//!         eprintln!("Connection failed: {}", e);
//!     }
//!     Err(QuicError::Timeout) => {
//!         eprintln!("Connection timed out");
//!     }
//!     Err(e) => {
//!         eprintln!("Other error: {}", e);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Integration with Landropic
//!
//! This crate integrates with other Landropic components:
//!
//! - [`landro-crypto`] - Device identity and certificate management
//! - [`landro-proto`] - Protocol buffer messages sent over QUIC streams
//! - [`landro-daemon`] - Service discovery and connection management
//! - [`landro-index`] - File metadata synchronization over control streams

pub mod client;
pub mod config;
pub mod connection;
pub mod errors;
pub mod pool;
pub mod protocol;
pub mod recovery;
pub mod resumable;
pub mod server;
pub mod transfer;

pub use client::QuicClient;
pub use config::QuicConfig;
pub use connection::{Connection, StreamType};
pub use errors::{QuicError, Result};
pub use pool::{ConnectionPool, PoolConfig, PooledConnection};
pub use protocol::{StreamProtocol, MessageType, ProtocolMessage, BatchTransferManager};
pub use recovery::{RetryPolicy, CircuitState, RecoveryClient, ConnectionHealthMonitor};
pub use resumable::{ResumableTransferManager, TransferCheckpoint, TransferStatus};
pub use server::QuicServer;
pub use transfer::{QuicTransferEngine, TransferProgress};
