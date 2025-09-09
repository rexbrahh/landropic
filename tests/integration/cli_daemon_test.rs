use std::process::Command;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, debug, error};
use anyhow::{Context, Result};

use crate::common::{
    init_crypto, TestDaemon, find_cli_binary, create_test_files
};

/// Test CLI can start and stop daemon successfully
#[tokio::test]
async fn test_cli_start_stop_daemon() -> Result<()> {
    init_crypto();
    
    let cli_binary = find_cli_binary()
        .context("CLI binary not found - build it with 'cargo build -p landro-cli'")?;
    
    info!("Testing CLI daemon start/stop with binary: {:?}", cli_binary);
    
    // Test daemon start
    let start_output = Command::new(&cli_binary)
        .arg("start")
        .arg("--storage")
        .arg("/tmp/landropic-test-cli")
        .arg("--port") 
        .arg("9999")
        .output()
        .context("Failed to execute CLI start command")?;
    
    info!("CLI start output: {}", String::from_utf8_lossy(&start_output.stdout));
    if !start_output.stderr.is_empty() {
        debug!("CLI start stderr: {}", String::from_utf8_lossy(&start_output.stderr));
    }
    
    // Give daemon time to start
    sleep(Duration::from_secs(2)).await;
    
    // Test daemon status
    let status_output = Command::new(&cli_binary)
        .arg("status")
        .output()
        .context("Failed to execute CLI status command")?;
    
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    info!("CLI status output: {}", status_str);
    
    // Status should indicate daemon is running
    assert!(
        status_str.contains("RUNNING") || status_str.contains("running"),
        "Daemon should be reported as running"
    );
    
    // Test daemon stop
    let stop_output = Command::new(&cli_binary)
        .arg("stop")
        .output()
        .context("Failed to execute CLI stop command")?;
    
    info!("CLI stop output: {}", String::from_utf8_lossy(&stop_output.stdout));
    
    // Give daemon time to stop
    sleep(Duration::from_secs(1)).await;
    
    // Test final status should show stopped
    let final_status_output = Command::new(&cli_binary)
        .arg("status")
        .output()
        .context("Failed to execute CLI final status command")?;
    
    let final_status_str = String::from_utf8_lossy(&final_status_output.stdout);
    info!("CLI final status output: {}", final_status_str);
    
    // Status should indicate daemon is stopped
    assert!(
        final_status_str.contains("STOPPED") || final_status_str.contains("not running"),
        "Daemon should be reported as stopped"
    );
    
    Ok(())
}

/// Test CLI can communicate with running daemon
#[tokio::test]
async fn test_cli_daemon_communication() -> Result<()> {
    init_crypto();
    
    let mut daemon = TestDaemon::new("test-comm-daemon")?;
    let port = daemon.start(None).await?;
    
    let cli_binary = find_cli_binary()
        .context("CLI binary not found")?;
    
    info!("Testing CLI communication with daemon on port {}", port);
    
    // Create test sync folder and files
    let sync_folder = daemon.sync_folder();
    tokio::fs::create_dir_all(&sync_folder).await?;
    let test_files = create_test_files(&sync_folder)?;
    
    info!("Created {} test files in {:?}", test_files.len(), sync_folder);
    
    // Test sync command (alpha feature)
    let sync_output = Command::new(&cli_binary)
        .arg("sync")
        .arg(format!("127.0.0.1:{}", port + 1)) // Different port to simulate peer
        .arg(&test_files[0])
        .output()
        .context("Failed to execute CLI sync command")?;
    
    let sync_str = String::from_utf8_lossy(&sync_output.stdout);
    info!("CLI sync output: {}", sync_str);
    
    // Should indicate sync was requested
    assert!(
        sync_str.contains("Sync requested") || sync_str.contains("Processing"),
        "CLI should indicate sync was requested"
    );
    
    daemon.stop().await?;
    Ok(())
}

/// Test CLI handles daemon not running scenario
#[tokio::test]
async fn test_cli_daemon_not_running() -> Result<()> {
    init_crypto();
    
    let cli_binary = find_cli_binary()
        .context("CLI binary not found")?;
    
    info!("Testing CLI behavior when daemon is not running");
    
    // Make sure no daemon is running by trying to stop first
    let _ = Command::new(&cli_binary)
        .arg("stop")
        .output();
    
    sleep(Duration::from_millis(500)).await;
    
    // Test status when daemon not running
    let status_output = Command::new(&cli_binary)
        .arg("status")
        .output()
        .context("Failed to execute CLI status command")?;
    
    let status_str = String::from_utf8_lossy(&status_output.stdout);
    info!("CLI status (daemon not running): {}", status_str);
    
    assert!(
        status_str.contains("STOPPED") || status_str.contains("not running"),
        "Should report daemon as not running"
    );
    
    // Test sync when daemon not running
    let sync_output = Command::new(&cli_binary)
        .arg("sync")
        .arg("127.0.0.1:9999")
        .arg("/tmp/test-file")
        .output()
        .context("Failed to execute CLI sync command")?;
    
    let sync_str = String::from_utf8_lossy(&sync_output.stdout);
    info!("CLI sync (daemon not running): {}", sync_str);
    
    assert!(
        sync_str.contains("not running") || sync_str.contains("Error"),
        "Should indicate daemon is not running"
    );
    
    Ok(())
}

/// Test multiple CLI commands in sequence
#[tokio::test]
async fn test_cli_command_sequence() -> Result<()> {
    init_crypto();
    
    let cli_binary = find_cli_binary()
        .context("CLI binary not found")?;
    
    info!("Testing CLI command sequence");
    
    // Clean slate - ensure no daemon running
    let _ = Command::new(&cli_binary)
        .arg("stop")
        .output();
    
    sleep(Duration::from_millis(500)).await;
    
    // Sequence: start -> status -> start (should be idempotent) -> stop -> status
    
    // 1. Start daemon
    let start1_output = Command::new(&cli_binary)
        .arg("start")
        .arg("--storage")
        .arg("/tmp/landropic-test-seq")
        .arg("--port")
        .arg("9998")
        .output()
        .context("Failed to start daemon first time")?;
    
    assert!(start1_output.status.success(), "First start should succeed");
    
    sleep(Duration::from_secs(2)).await;
    
    // 2. Check status
    let status1_output = Command::new(&cli_binary)
        .arg("status")
        .output()
        .context("Failed to check status")?;
    
    assert!(status1_output.status.success(), "Status check should succeed");
    let status1_str = String::from_utf8_lossy(&status1_output.stdout);
    assert!(
        status1_str.contains("RUNNING") || status1_str.contains("running"),
        "Should show daemon running"
    );
    
    // 3. Try to start again (should be idempotent)
    let start2_output = Command::new(&cli_binary)
        .arg("start")
        .arg("--storage")
        .arg("/tmp/landropic-test-seq")
        .arg("--port")
        .arg("9998")
        .output()
        .context("Failed to start daemon second time")?;
    
    // This should either succeed (idempotent) or give a clear "already running" message
    let start2_str = String::from_utf8_lossy(&start2_output.stdout);
    info!("Second start output: {}", start2_str);
    
    // 4. Stop daemon
    let stop_output = Command::new(&cli_binary)
        .arg("stop")
        .output()
        .context("Failed to stop daemon")?;
    
    assert!(stop_output.status.success(), "Stop should succeed");
    
    sleep(Duration::from_secs(1)).await;
    
    // 5. Final status check
    let status2_output = Command::new(&cli_binary)
        .arg("status")
        .output()
        .context("Failed to check final status")?;
    
    let status2_str = String::from_utf8_lossy(&status2_output.stdout);
    assert!(
        status2_str.contains("STOPPED") || status2_str.contains("not running"),
        "Should show daemon stopped"
    );
    
    Ok(())
}

/// Test CLI with invalid arguments
#[tokio::test]
async fn test_cli_invalid_arguments() -> Result<()> {
    init_crypto();
    
    let cli_binary = find_cli_binary()
        .context("CLI binary not found")?;
    
    info!("Testing CLI with invalid arguments");
    
    // Test invalid port
    let invalid_port_output = Command::new(&cli_binary)
        .arg("start")
        .arg("--port")
        .arg("999999") // Invalid port number
        .output()
        .context("Failed to test invalid port")?;
    
    // Should fail or handle gracefully
    let port_output_str = String::from_utf8_lossy(&invalid_port_output.stdout);
    let port_error_str = String::from_utf8_lossy(&invalid_port_output.stderr);
    
    info!("Invalid port stdout: {}", port_output_str);
    info!("Invalid port stderr: {}", port_error_str);
    
    // Test invalid storage path (permission denied scenario)
    let invalid_storage_output = Command::new(&cli_binary)
        .arg("start")
        .arg("--storage")
        .arg("/root/forbidden") // Should fail due to permissions on most systems
        .arg("--port")
        .arg("9997")
        .output()
        .context("Failed to test invalid storage")?;
    
    let storage_output_str = String::from_utf8_lossy(&invalid_storage_output.stdout);
    let storage_error_str = String::from_utf8_lossy(&invalid_storage_output.stderr);
    
    info!("Invalid storage stdout: {}", storage_output_str);
    info!("Invalid storage stderr: {}", storage_error_str);
    
    // Test invalid sync peer address
    let invalid_sync_output = Command::new(&cli_binary)
        .arg("sync")
        .arg("invalid-address") // Invalid address format
        .arg("/tmp/nonexistent")
        .output()
        .context("Failed to test invalid sync")?;
    
    let sync_output_str = String::from_utf8_lossy(&invalid_sync_output.stdout);
    info!("Invalid sync output: {}", sync_output_str);
    
    Ok(())
}