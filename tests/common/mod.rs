use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};
use std::fs;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use anyhow::{Context, Result};
use tracing::{info, warn, error};

static CRYPTO_INIT: Once = Once::new();

/// Initialize crypto provider for tests - call once per test process
pub fn init_crypto() {
    CRYPTO_INIT.call_once(|| {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
    });
}

/// Test daemon instance manager
pub struct TestDaemon {
    pub child: Option<Child>,
    pub port: u16,
    pub storage_path: PathBuf,
    pub device_name: String,
    pub temp_dir: TempDir,
}

impl TestDaemon {
    /// Create a new test daemon instance
    pub fn new(device_name: &str) -> Result<Self> {
        let temp_dir = TempDir::new().context("Failed to create temp directory")?;
        let storage_path = temp_dir.path().join("storage");
        fs::create_dir_all(&storage_path)?;

        Ok(TestDaemon {
            child: None,
            port: 0, // Will be set when started
            storage_path,
            device_name: device_name.to_string(),
            temp_dir,
        })
    }

    /// Start the daemon with optional port (0 for auto-assign)
    pub async fn start(&mut self, port: Option<u16>) -> Result<u16> {
        if self.child.is_some() {
            return Err(anyhow::anyhow!("Daemon already running"));
        }

        // Find daemon binary
        let daemon_binary = find_daemon_binary()?;
        
        let assigned_port = port.unwrap_or_else(|| find_free_port());
        
        info!("Starting test daemon on port {} with storage {:?}", assigned_port, self.storage_path);
        
        let child = Command::new(&daemon_binary)
            .arg("--storage")
            .arg(&self.storage_path)
            .arg("--port")
            .arg(assigned_port.to_string())
            .arg("--device-name")
            .arg(&self.device_name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn daemon process")?;

        self.child = Some(child);
        self.port = assigned_port;

        // Wait for daemon to be ready
        self.wait_for_ready().await?;
        
        info!("Test daemon started successfully on port {}", assigned_port);
        Ok(assigned_port)
    }

    /// Stop the daemon
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            info!("Stopping test daemon (PID: {})", child.id());
            
            // Try graceful termination first
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
            }
            #[cfg(windows)]
            {
                let _ = child.kill();
            }

            // Wait for process to exit
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Ok(Some(_)) = child.try_wait() {
                    info!("Test daemon stopped gracefully");
                    return Ok(());
                }
                sleep(Duration::from_millis(100)).await;
            }

            // Force kill if still running
            warn!("Force killing test daemon");
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    /// Wait for daemon to be ready to accept connections
    async fn wait_for_ready(&self) -> Result<()> {
        let start = Instant::now();
        let max_wait = Duration::from_secs(10);
        
        while start.elapsed() < max_wait {
            // Try to check if daemon is listening (basic connectivity check)
            if self.is_listening().await {
                return Ok(());
            }
            sleep(Duration::from_millis(200)).await;
        }
        
        Err(anyhow::anyhow!("Daemon failed to become ready within timeout"))
    }

    /// Check if daemon is listening on its port
    async fn is_listening(&self) -> bool {
        use std::net::TcpStream;
        use std::net::SocketAddr;
        
        let addr: SocketAddr = format!("127.0.0.1:{}", self.port).parse().unwrap();
        
        // Try to connect - daemon should be listening even if it rejects the connection
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(_) => true,
            Err(_) => {
                // Check if something is listening on the port at all
                match TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
                    Ok(_) => true,
                    Err(e) => {
                        match e.kind() {
                            std::io::ErrorKind::ConnectionRefused => true, // Something is listening but refusing
                            _ => false,
                        }
                    }
                }
            }
        }
    }

    /// Get the daemon's address
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Get a path within the daemon's storage directory
    pub fn storage_file(&self, filename: &str) -> PathBuf {
        self.storage_path.join(filename)
    }

    /// Get the sync folder path for this daemon
    pub fn sync_folder(&self) -> PathBuf {
        self.temp_dir.path().join("sync")
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Find the daemon binary in build directories
pub fn find_daemon_binary() -> Result<PathBuf> {
    let possible_paths = [
        "target/release/landro-daemon",
        "target/debug/landro-daemon",
        "landro-daemon/target/release/landro-daemon", 
        "landro-daemon/target/debug/landro-daemon",
        "../target/release/landro-daemon",
        "../target/debug/landro-daemon",
    ];

    for path_str in &possible_paths {
        let path = PathBuf::from(path_str);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow::anyhow!(
        "Could not find landro-daemon binary. Build it first with: cargo build -p landro-daemon"
    ))
}

/// Find the CLI binary in build directories  
pub fn find_cli_binary() -> Result<PathBuf> {
    let possible_paths = [
        "target/release/landro-cli",
        "target/debug/landro-cli",
        "landro-cli/target/release/landro-cli",
        "landro-cli/target/debug/landro-cli", 
        "../target/release/landro-cli",
        "../target/debug/landro-cli",
    ];

    for path_str in &possible_paths {
        let path = PathBuf::from(path_str);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow::anyhow!(
        "Could not find landro-cli binary. Build it first with: cargo build -p landro-cli"
    ))
}

/// Find a free port for testing
pub fn find_free_port() -> u16 {
    use std::net::{TcpListener, SocketAddr};
    
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to find free port");
    let addr = listener.local_addr().expect("Failed to get local address");
    let port = addr.port();
    drop(listener);
    port
}

/// Create a test file hierarchy for sync testing
pub fn create_test_files(base_path: &Path) -> Result<Vec<PathBuf>> {
    let mut created_files = Vec::new();
    
    // Create directory structure
    let dirs = [
        "documents",
        "documents/projects", 
        "media",
        "media/images",
        "temp",
    ];
    
    for dir in &dirs {
        let dir_path = base_path.join(dir);
        fs::create_dir_all(&dir_path)?;
    }

    // Create test files with various sizes and content
    let files = [
        ("README.txt", "This is a test file for landropic sync testing.\nIt contains some basic text content."),
        ("documents/project.md", "# Test Project\n\nThis is a markdown document for testing sync functionality.\n\n## Features\n- File synchronization\n- Encryption\n- Cross-platform support"),
        ("documents/projects/alpha.rs", "fn main() {\n    println!(\"Hello from Landropic alpha!\");\n}\n"),
        ("media/test.dat", &"x".repeat(1024)), // 1KB file
        ("temp/large.bin", &"0123456789".repeat(5000)), // ~50KB file
    ];

    for (filename, content) in &files {
        let file_path = base_path.join(filename);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, content)?;
        created_files.push(file_path);
    }

    Ok(created_files)
}

/// Generate test data of specified size
pub fn generate_test_data(size: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size);
    let pattern = b"LANDROPIC_TEST_DATA_";
    
    for i in 0..size {
        data.push(pattern[i % pattern.len()]);
    }
    
    data
}

/// Compare directory contents for sync verification
pub fn compare_directories(dir1: &Path, dir2: &Path) -> Result<bool> {
    let files1 = collect_files_recursive(dir1)?;
    let files2 = collect_files_recursive(dir2)?;
    
    if files1.len() != files2.len() {
        error!("Directory file counts differ: {} vs {}", files1.len(), files2.len());
        return Ok(false);
    }
    
    for (rel_path, content1) in files1 {
        if let Some(content2) = files2.get(&rel_path) {
            if content1 != *content2 {
                error!("File content differs: {}", rel_path.display());
                return Ok(false);
            }
        } else {
            error!("File missing in dir2: {}", rel_path.display());
            return Ok(false);
        }
    }
    
    Ok(true)
}

/// Recursively collect all files in a directory with their relative paths and content
fn collect_files_recursive(dir: &Path) -> Result<std::collections::HashMap<PathBuf, Vec<u8>>> {
    let mut files = std::collections::HashMap::new();
    
    if !dir.exists() {
        return Ok(files);
    }
    
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_file() {
            let rel_path = path.strip_prefix(dir)?.to_path_buf();
            let content = fs::read(path)?;
            files.insert(rel_path, content);
        }
    }
    
    Ok(files)
}