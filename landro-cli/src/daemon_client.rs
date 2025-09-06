use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tokio::time::timeout;

const DAEMON_BASE_URL: &str = "http://127.0.0.1:7703";
const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP client for communicating with the landropic daemon
pub struct Client {
    client: reqwest::Client,
    base_url: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(DAEMON_TIMEOUT)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url: DAEMON_BASE_URL.to_string(),
        })
    }

    /// Get pairing information (QR code data)
    pub async fn get_pairing_info(&self) -> Result<String> {
        let response = self
            .client
            .get(&format!("{}/api/pairing/info", self.base_url))
            .send()
            .await
            .context("Failed to get pairing info")?;

        let pairing_response: PairingInfoResponse = response
            .json()
            .await
            .context("Failed to parse pairing info response")?;

        Ok(pairing_response.pairing_code)
    }

    /// Wait for another device to pair with us
    pub async fn wait_for_pairing(&self) -> Result<()> {
        // Poll the pairing status endpoint until pairing completes
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 120; // 2 minutes at 1 second intervals

        loop {
            if attempts >= MAX_ATTEMPTS {
                return Err(anyhow!("Pairing timeout after 2 minutes"));
            }

            let response = self
                .client
                .get(&format!("{}/api/pairing/status", self.base_url))
                .send()
                .await
                .context("Failed to check pairing status")?;

            let status: PairingStatusResponse = response
                .json()
                .await
                .context("Failed to parse pairing status")?;

            if status.paired {
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
            attempts += 1;
        }
    }

    /// Pair with another device using a pairing code
    pub async fn pair_with_code(&self, code: &str) -> Result<()> {
        let request = PairRequest {
            pairing_code: code.to_string(),
        };

        let response = self
            .client
            .post(&format!("{}/api/pairing/pair", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to send pairing request")?;

        if response.status().is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();
            Err(anyhow!("Pairing failed: {}", error_text))
        }
    }

    /// Add a folder to sync
    pub async fn add_sync_folder(&self, folder: &PathBuf, watch: bool) -> Result<()> {
        let request = AddFolderRequest {
            path: folder.to_string_lossy().to_string(),
            watch,
        };

        let response = self
            .client
            .post(&format!("{}/api/sync/add", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to add sync folder")?;

        if response.status().is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();
            Err(anyhow!("Failed to add sync folder: {}", error_text))
        }
    }

    /// Remove a folder from sync
    pub async fn remove_sync_folder(&self, folder: &PathBuf) -> Result<()> {
        let request = RemoveFolderRequest {
            path: folder.to_string_lossy().to_string(),
        };

        let response = self
            .client
            .post(&format!("{}/api/sync/remove", self.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to remove sync folder")?;

        if response.status().is_success() {
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_default();
            Err(anyhow!("Failed to remove sync folder: {}", error_text))
        }
    }

    /// Get daemon and sync status
    pub async fn get_status(&self, folder: Option<PathBuf>) -> Result<StatusResponse> {
        let mut url = format!("{}/api/status", self.base_url);

        if let Some(folder_path) = folder {
            url.push_str(&format!(
                "?folder={}",
                urlencoding::encode(&folder_path.to_string_lossy())
            ));
        }

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get daemon status")?;

        response
            .json()
            .await
            .context("Failed to parse status response")
    }

    /// List all synced folders
    pub async fn list_folders(&self) -> Result<Vec<String>> {
        let response = self
            .client
            .get(&format!("{}/api/sync/list", self.base_url))
            .send()
            .await
            .context("Failed to list sync folders")?;

        let list_response: ListFoldersResponse = response
            .json()
            .await
            .context("Failed to parse folder list response")?;

        Ok(list_response.folders)
    }

    /// Get list of paired peers
    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let response = self
            .client
            .get(&format!("{}/api/peers", self.base_url))
            .send()
            .await
            .context("Failed to get peers list")?;

        let peers_response: PeersResponse = response
            .json()
            .await
            .context("Failed to parse peers response")?;

        Ok(peers_response.peers)
    }

    /// Get sync progress for a specific folder or all folders
    pub async fn get_sync_progress(
        &self,
        folder: Option<&std::path::PathBuf>,
    ) -> Result<SyncProgressResponse> {
        let mut url = format!("{}/api/sync/progress", self.base_url);

        if let Some(folder_path) = folder {
            url.push_str(&format!(
                "?folder={}",
                urlencoding::encode(&folder_path.to_string_lossy())
            ));
        }

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get sync progress")?;

        response
            .json()
            .await
            .context("Failed to parse sync progress response")
    }
}

/// Check if the daemon is running
pub async fn is_running() -> bool {
    let client = match Client::new() {
        Ok(client) => client,
        Err(_) => return false,
    };

    // Try to connect with a short timeout
    timeout(Duration::from_secs(2), async {
        client
            .client
            .get(&format!("{}/api/health", client.base_url))
            .send()
            .await
    })
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Start the daemon process
pub async fn start_daemon() -> Result<()> {
    if is_running().await {
        return Ok(());
    }

    // Find the daemon binary - try multiple locations
    let daemon_binary = find_daemon_binary().context("Could not find landropic daemon binary")?;

    // Start the daemon in detached mode
    let mut cmd = Command::new(&daemon_binary);
    cmd.arg("--daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        // Use setsid on Unix to detach from the terminal
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    cmd.spawn().context("Failed to start daemon process")?;

    // Wait for daemon to start up
    let mut attempts = 0;
    while attempts < 30 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if is_running().await {
            return Ok(());
        }
        attempts += 1;
    }

    Err(anyhow!("Daemon failed to start within 15 seconds"))
}

/// Stop the daemon process
pub async fn stop_daemon() -> Result<()> {
    if !is_running().await {
        return Ok(());
    }

    let client = Client::new()?;
    let response = client
        .client
        .post(&format!("{}/api/shutdown", client.base_url))
        .send()
        .await
        .context("Failed to send shutdown request")?;

    if response.status().is_success() {
        // Wait for daemon to shut down
        let mut attempts = 0;
        while attempts < 30 && is_running().await {
            tokio::time::sleep(Duration::from_millis(500)).await;
            attempts += 1;
        }
        Ok(())
    } else {
        Err(anyhow!("Failed to stop daemon"))
    }
}

/// Find the daemon binary in various locations
fn find_daemon_binary() -> Option<PathBuf> {
    // Possible locations for the daemon binary
    let candidates = [
        // Same directory as CLI
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("landro-daemon"),
        // Target directory for development
        PathBuf::from("target/debug/landro-daemon"),
        PathBuf::from("target/release/landro-daemon"),
        // System paths
        PathBuf::from("/usr/local/bin/landro-daemon"),
        PathBuf::from("/usr/bin/landro-daemon"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    None
}

// API Request/Response types

#[derive(Serialize)]
struct PairRequest {
    pairing_code: String,
}

#[derive(Deserialize)]
struct PairingInfoResponse {
    pairing_code: String,
}

#[derive(Deserialize)]
struct PairingStatusResponse {
    paired: bool,
}

#[derive(Serialize)]
struct AddFolderRequest {
    path: String,
    watch: bool,
}

#[derive(Serialize)]
struct RemoveFolderRequest {
    path: String,
}

#[derive(Deserialize)]
struct ListFoldersResponse {
    folders: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub device_name: String,
    pub daemon_status: String,
    pub connected_peers: u32,
    pub sync_folders: Vec<FolderStatus>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FolderStatus {
    pub path: String,
    pub status: String,
    pub files_synced: u32,
    pub files_pending: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub device_id: String,
    pub name: String,
    pub connected: bool,
    pub last_seen: Option<String>,
}

#[derive(Deserialize)]
struct PeersResponse {
    peers: Vec<PeerInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncProgressResponse {
    pub folder: String,
    pub total_files: u64,
    pub files_completed: u64,
    pub total_bytes: u64,
    pub bytes_completed: u64,
    pub current_file: Option<String>,
    pub phase: String, // "scanning", "uploading", "downloading", "complete"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_binary_search() {
        // This test will only pass in development environments
        // where we have the target directory structure
        let binary = find_daemon_binary();
        // Don't assert anything specific since the binary location
        // varies by environment, just ensure the function doesn't panic
        println!("Found daemon binary: {:?}", binary);
    }

    #[test]
    fn test_client_creation() {
        let client = Client::new();
        assert!(client.is_ok(), "Should be able to create HTTP client");
    }
}
