use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Main daemon orchestrator
pub struct Daemon {
    running: Arc<RwLock<bool>>,
}

impl Daemon {
    /// Create a new daemon instance
    pub fn new() -> Self {
        Self {
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the daemon
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut running = self.running.write().await;
        if *running {
            return Err("Daemon already running".into());
        }

        *running = true;
        info!("Landropic daemon started");

        // TODO: Start all subsystems
        // - QUIC server
        // - mDNS discovery
        // - File watcher
        // - Sync engine

        Ok(())
    }

    /// Stop the daemon
    pub async fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut running = self.running.write().await;
        if !*running {
            return Err("Daemon not running".into());
        }

        *running = false;
        info!("Landropic daemon stopped");

        Ok(())
    }

    /// Check if daemon is running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}
