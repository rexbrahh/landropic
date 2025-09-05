/// Discovery service demonstration
/// 
/// This example shows how to use the mDNS discovery service to:
/// - Advertise the local landropic service
/// - Discover other landropic peers on the network
/// - Handle peer events (discovery, removal, updates)
/// 
/// Run with: cargo run --example discovery_demo
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::time::sleep;
use tracing::{info, warn, error};
use tracing_subscriber;

use landro_crypto::DeviceIdentity;
use landro_daemon::discovery::{Discovery, DiscoveryEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    
    info!("Starting Landropic Discovery Demo");
    
    // Create device identity
    let device_identity = Arc::new(DeviceIdentity::generate("Demo Device")?);
    info!("Device ID: {}", device_identity.device_id());
    info!("Device Name: {}", device_identity.device_name());
    
    // Create discovery service
    let mut discovery = Discovery::new(device_identity);
    
    // Subscribe to discovery events
    let mut event_receiver = discovery.subscribe();
    
    // Spawn task to handle discovery events
    let event_handler = tokio::spawn(async move {
        while let Ok(event) = event_receiver.recv().await {
            match event {
                DiscoveryEvent::PeerDiscovered(peer) => {
                    info!("🔍 Discovered peer: {} ({}) at {}:{}", 
                          peer.device_name, peer.device_id, peer.address, peer.port);
                    info!("   Capabilities: {:?}", peer.capabilities);
                    info!("   Protocol Version: {}", peer.protocol_version);
                }
                DiscoveryEvent::PeerRemoved(device_id) => {
                    info!("👋 Peer removed: {}", device_id);
                }
                DiscoveryEvent::PeerUpdated(peer) => {
                    info!("📝 Peer updated: {} ({}) at {}:{}", 
                          peer.device_name, peer.device_id, peer.address, peer.port);
                }
                DiscoveryEvent::ServiceRegistered => {
                    info!("✅ Our service has been registered on the network");
                }
                DiscoveryEvent::ServiceUnregistered => {
                    info!("❌ Our service has been unregistered from the network");
                }
                DiscoveryEvent::NetworkError(error) => {
                    warn!("⚠️  Network error: {}", error);
                }
            }
        }
    });
    
    // Initialize the discovery service
    if let Err(e) = discovery.initialize().await {
        error!("Failed to initialize discovery service: {}", e);
        return Err(e.into());
    }
    
    // Start advertising our service
    let port = 12345;
    let capabilities = vec![
        "sync".to_string(),
        "backup".to_string(),
        "v1".to_string(),
    ];
    
    info!("🚀 Starting service advertisement on port {}", port);
    if let Err(e) = discovery.start_advertising(port, capabilities).await {
        error!("Failed to start advertising: {}", e);
        return Err(e.into());
    }
    
    // Start browsing for peers
    info!("🔍 Starting peer discovery");
    if let Err(e) = discovery.start_browsing().await {
        error!("Failed to start browsing: {}", e);
        return Err(e.into());
    }
    
    info!("Discovery service is now running. Press Ctrl+C to stop.");
    
    // Create a periodic task to show current peers
    // We'll use a simpler approach and just show peers once after a delay
    tokio::spawn(async move {
        sleep(Duration::from_secs(10)).await;
        info!("📊 Waiting for peer discovery... (this demo will show discovered peers periodically)");
        
        // In a real implementation, you would access the discovery service's peer list
        // through shared state or channels. For this demo, we'll just show the concept.
        loop {
            sleep(Duration::from_secs(30)).await;
            info!("📊 (Peer status would be shown here in a real implementation)");
        }
    });
    
    // Wait for shutdown signal
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Received shutdown signal, stopping discovery service...");
        }
        Err(err) => {
            error!("Unable to listen for shutdown signal: {}", err);
        }
    }
    
    // The periodic task will be automatically cancelled when the program exits
    event_handler.abort();
    
    // Stop advertising
    if let Err(e) = discovery.stop_advertising().await {
        warn!("Error stopping advertising: {}", e);
    }
    
    // Shutdown the discovery service
    if let Err(e) = discovery.shutdown().await {
        warn!("Error shutting down discovery service: {}", e);
    }
    
    info!("Discovery demo stopped");
    Ok(())
}