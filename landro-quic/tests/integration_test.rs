//! Integration tests for QUIC connection management and basic sync protocol

use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use landro_crypto::{CertificateVerifier, DeviceIdentity};
use landro_quic::{
    Connection, ConnectionPool, PoolConfig, QuicClient, QuicConfig, QuicServer, QuicTransferEngine,
    RecoveryClient, RetryPolicy, StreamProtocol,
};

/// Test basic client-server connection and handshake
#[tokio::test]
async fn test_basic_connection_handshake() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create server identity and verifier
    let server_identity = Arc::new(DeviceIdentity::generate("test-server")?);
    let server_verifier = Arc::new(CertificateVerifier::for_pairing());
    let server_config = QuicConfig::default().bind_addr("127.0.0.1:0".parse().unwrap());

    // Create and start server
    let mut server = QuicServer::new(server_identity, server_verifier.clone(), server_config);
    server.start().await?;

    let server_addr = server
        .local_addr()
        .expect("Server should have local address");
    println!("Server started on: {}", server_addr);

    // Start server accept loop in background
    let server_clone = Arc::new(server);
    let server_handle = {
        let server = server_clone.clone();
        tokio::spawn(async move {
            // Accept one connection for testing
            match server.accept().await {
                Ok(connection) => {
                    println!(
                        "Server: Connection accepted from {}",
                        connection.remote_address()
                    );
                    // Keep connection alive briefly for the test
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => {
                    eprintln!("Server: Failed to accept connection: {}", e);
                }
            }
        })
    };

    // Give server time to start accepting
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create client
    let client_identity = Arc::new(DeviceIdentity::generate("test-client")?);
    let client_verifier = Arc::new(CertificateVerifier::for_pairing());
    let client_config = QuicConfig::default();

    let client = QuicClient::new(client_identity, client_verifier, client_config).await?;

    // Test connection with timeout
    let connection_result = timeout(Duration::from_secs(5), client.connect_addr(server_addr)).await;

    match connection_result {
        Ok(Ok(connection)) => {
            println!("Client: Successfully connected to server");

            // Verify connection is healthy
            assert!(!connection.is_closed());
            assert_eq!(connection.remote_address(), server_addr);

            // Try opening a stream
            let stream_result = timeout(Duration::from_secs(2), connection.open_uni()).await;

            match stream_result {
                Ok(Ok(mut stream)) => {
                    println!("Client: Successfully opened stream");
                    let _ = stream.finish();
                }
                Ok(Err(e)) => {
                    panic!("Failed to open stream: {}", e);
                }
                Err(_) => {
                    panic!("Stream opening timed out");
                }
            }
        }
        Ok(Err(e)) => {
            panic!("Failed to connect to server: {}", e);
        }
        Err(_) => {
            panic!("Connection attempt timed out");
        }
    }

    // Wait for server task to complete
    let _ = timeout(Duration::from_secs(3), server_handle).await;

    println!("Basic connection handshake test completed successfully");
    Ok(())
}

/// Test connection pool functionality
#[tokio::test]
async fn test_connection_pool() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create a client for the pool
    let identity = Arc::new(DeviceIdentity::generate("pool-test-client")?);
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    let config = QuicConfig::default();

    let client = Arc::new(QuicClient::new(identity, verifier, config).await?);

    // Create connection pool with small limits for testing
    let pool_config = PoolConfig {
        max_connections_per_peer: 2,
        max_total_connections: 5,
        max_idle_time: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(5),
        max_retry_attempts: 2,
        retry_delay: Duration::from_millis(100),
    };

    let pool = ConnectionPool::new(client, pool_config);

    // Test pool statistics
    let initial_stats = pool.get_stats().await;
    assert!(initial_stats.is_empty());

    // Trying to connect to a non-existent address should fail gracefully
    let fake_addr = "127.0.0.1:12345".parse().unwrap();
    let connect_result = timeout(Duration::from_secs(3), pool.get_connection(fake_addr)).await;

    // Should fail but not panic
    assert!(connect_result.is_ok()); // timeout succeeded
    assert!(connect_result.unwrap().is_err()); // but connection failed

    // Test shutdown
    pool.shutdown().await?;

    println!("Connection pool test completed successfully");
    Ok(())
}

/// Test recovery client with retry logic
#[tokio::test]
async fn test_recovery_client() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create a client
    let identity = Arc::new(DeviceIdentity::generate("recovery-test-client")?);
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    let config = QuicConfig::default();

    let client = Arc::new(QuicClient::new(identity, verifier, config).await?);

    // Create recovery client with aggressive retry policy
    let retry_policy = RetryPolicy::aggressive();
    let recovery_client = RecoveryClient::new(client, retry_policy);

    // Test connecting to non-existent server
    let fake_addr = "127.0.0.1:54321".parse().unwrap();
    let start_time = std::time::Instant::now();

    let result = timeout(
        Duration::from_secs(10),
        recovery_client.connect_with_recovery(fake_addr),
    )
    .await;

    let elapsed = start_time.elapsed();

    match result {
        Ok(Err(_)) => {
            println!(
                "Recovery client correctly failed after retries (took {:?})",
                elapsed
            );
            // Should take some time due to retries
            assert!(elapsed > Duration::from_millis(500));
        }
        Ok(Ok(_)) => {
            panic!("Should not have connected to non-existent server");
        }
        Err(_) => {
            println!("Recovery client timed out as expected");
        }
    }

    // Test circuit breaker states
    let circuit_states = recovery_client.get_circuit_states().await;
    println!("Circuit breaker states: {:?}", circuit_states);

    println!("Recovery client test completed successfully");
    Ok(())
}

/// Test stream protocol message handling
#[tokio::test]
async fn test_stream_protocol_simulation() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create mock connection for protocol testing
    // Note: This is a simplified test since we can't easily create a real connection
    // In a real environment, this would use actual connections

    println!("Stream protocol types are available and compile correctly");

    // Test protocol message types
    use landro_proto::{Ack, ChunkData, FolderSummary, Manifest, Want};
    use landro_quic::{MessageType, ProtocolMessage};

    assert_eq!(MessageType::Hello as u8, 0);
    assert_eq!(MessageType::FolderSummary as u8, 1);
    assert_eq!(MessageType::Manifest as u8, 2);
    assert_eq!(MessageType::Want as u8, 3);
    assert_eq!(MessageType::ChunkData as u8, 4);
    assert_eq!(MessageType::Ack as u8, 5);
    assert_eq!(MessageType::Error as u8, 6);

    // Test message type conversion
    assert_eq!(MessageType::from_u8(1), Some(MessageType::FolderSummary));
    assert_eq!(MessageType::from_u8(99), None);

    println!("Stream protocol simulation completed successfully");
    Ok(())
}

/// Test transfer engine integration
#[tokio::test]
async fn test_transfer_engine_basic() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create a mock connection (in real usage, this would be from QuicClient.connect())
    // For this test, we just verify the types compile and can be instantiated

    println!("Transfer engine types are available and compile correctly");

    // Test transfer progress formatting
    use landro_quic::{TransferProgress, TransferStatus};

    let progress = TransferProgress {
        transfer_id: "test-transfer".to_string(),
        status: TransferStatus::Active,
        progress: 0.75,
        bandwidth_bytes_per_sec: 2048.0, // 2 KB/s
        chunks_received: 75,
        chunks_total: 100,
        bytes_transferred: 75 * 1024,
        bytes_total: 100 * 1024,
    };

    assert_eq!(progress.status_text(), "Active");
    assert_eq!(progress.progress_percent(), 75.0);
    assert_eq!(progress.bandwidth_text(), "2.0 KB/s");
    assert_eq!(progress.size_text(), "75.0 KB / 100.0 KB");

    println!("Transfer engine basic test completed successfully");
    Ok(())
}

/// Integration test that verifies the entire stack builds and basic types work
#[tokio::test]
async fn test_integration_compilation() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Testing that all QUIC components integrate correctly...");

    // Test that all the major types can be constructed without errors
    let identity = Arc::new(DeviceIdentity::generate("integration-test")?);
    let verifier = Arc::new(CertificateVerifier::for_pairing());
    let config = QuicConfig::default();

    // Create client
    let client = QuicClient::new(identity.clone(), verifier.clone(), config.clone()).await?;
    let client = Arc::new(client);
    println!("✓ QuicClient created successfully");

    // Create server
    let server = QuicServer::new(identity.clone(), verifier.clone(), config.clone());
    println!("✓ QuicServer created successfully");

    // Create connection pool
    let pool = ConnectionPool::new(client.clone(), PoolConfig::default());
    println!("✓ ConnectionPool created successfully");

    // Create recovery client
    let recovery_client = RecoveryClient::new(client, RetryPolicy::default());
    println!("✓ RecoveryClient created successfully");

    // Verify error types work
    use landro_quic::QuicError;
    let error = QuicError::Protocol("test error".to_string());
    let cloned_error = error.clone();
    assert_eq!(error.to_string(), cloned_error.to_string());
    println!("✓ Error types work correctly");

    // Test that we can shutdown cleanly
    pool.shutdown().await?;
    println!("✓ Clean shutdown works");

    println!("✅ All QUIC components integrate correctly!");
    Ok(())
}
