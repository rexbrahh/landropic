use clap::{Parser, Subcommand};
use std::process::{Command, Stdio};
use std::path::{Path, PathBuf};
use std::fs;
use anyhow::{Result, Context};

#[derive(Parser)]
#[command(name = "landro")]
#[command(version = "0.0.1-alpha")]
#[command(about = "Landropic encrypted file sync - CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the sync daemon
    Start {
        #[arg(short, long, default_value = "~/.landropic")]
        storage: String,
        
        #[arg(short, long, default_value = "9876")]
        port: u16,
    },
    
    /// Stop the running daemon
    Stop,
    
    /// Show daemon status
    Status,
    
    /// Sync files to a peer (alpha test feature)
    Sync {
        /// Peer address (e.g., 192.168.1.100:9876)
        peer: String,
        
        /// File or directory to sync
        path: PathBuf,
    },
}

// PID file management
const PID_FILE: &str = "/tmp/landro-daemon.pid";

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Start { storage, port } => start_daemon(storage, port),
        Commands::Stop => stop_daemon(),
        Commands::Status => show_status(),
        Commands::Sync { peer, path } => trigger_sync(peer, path),
    }
}

fn start_daemon(storage: String, port: u16) -> Result<()> {
    // Check if already running
    if daemon_running()? {
        println!("✅ Daemon already running");
        return Ok(());
    }
    
    // Expand home directory
    let storage_path = shellexpand::tilde(&storage).to_string();
    
    // Find daemon binary - try both release and debug builds
    let daemon_path = find_daemon_binary()?;
    
    // Start daemon in background
    let child = Command::new(&daemon_path)
        .arg("--storage").arg(&storage_path)
        .arg("--port").arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to start daemon")?;
    
    // Save PID
    let pid = child.id();
    fs::write(PID_FILE, pid.to_string())?;
    
    println!("🚀 Daemon started (PID: {})", pid);
    println!("   Storage: {}", storage_path);
    println!("   Port: {}", port);
    
    Ok(())
}

fn stop_daemon() -> Result<()> {
    if !daemon_running()? {
        println!("ℹ️  Daemon not running");
        return Ok(());
    }
    
    let pid = fs::read_to_string(PID_FILE)?;
    let pid: u32 = pid.trim().parse()?;
    
    // Send SIGTERM (cross-platform approach)
    #[cfg(unix)]
    {
        // Use kill command on Unix systems
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .output()?;
    }
    
    #[cfg(windows)]
    {
        // Use taskkill on Windows
        Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .output()?;
    }
    
    // Wait a moment
    std::thread::sleep(std::time::Duration::from_secs(1));
    
    // Check if stopped
    if !daemon_running()? {
        fs::remove_file(PID_FILE).ok();
        println!("✅ Daemon stopped");
    } else {
        // Force kill if needed
        #[cfg(unix)]
        {
            Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .output()?;
        }
        
        #[cfg(windows)]
        {
            Command::new("taskkill")
                .arg("/PID")
                .arg(pid.to_string())
                .arg("/F")
                .output()?;
        }
        
        fs::remove_file(PID_FILE).ok();
        println!("⚠️  Daemon force stopped");
    }
    
    Ok(())
}

fn daemon_running() -> Result<bool> {
    if !Path::new(PID_FILE).exists() {
        return Ok(false);
    }
    
    let pid = fs::read_to_string(PID_FILE)?;
    let pid: u32 = pid.trim().parse()?;
    
    // Check if process exists (cross-platform)
    #[cfg(unix)]
    {
        let output = Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .output()?;
        
        Ok(output.status.success())
    }
    
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .arg("/FI")
            .arg(&format!("PID eq {}", pid))
            .output()?;
            
        let output_str = String::from_utf8_lossy(&output.stdout);
        Ok(output_str.contains(&pid.to_string()))
    }
}

fn show_status() -> Result<()> {
    if daemon_running()? {
        let pid = fs::read_to_string(PID_FILE)?;
        println!("✅ Daemon: RUNNING (PID: {})", pid.trim());
        
        // Try to get more info (future: via HTTP API)
        println!("   Port: 9876 (default)");
        println!("   Peers: 0 connected (alpha)");
    } else {
        println!("❌ Daemon: STOPPED");
    }
    
    Ok(())
}

fn trigger_sync(peer: String, path: PathBuf) -> Result<()> {
    if !daemon_running()? {
        println!("❌ Error: Daemon not running. Start with 'landro start'");
        return Ok(());
    }
    
    // For alpha: Write sync request to a file that daemon monitors
    let sync_request = format!("{}|{}", peer, path.display());
    fs::write("/tmp/landro-sync-request", sync_request)?;
    
    println!("📤 Sync requested:");
    println!("   File: {}", path.display());
    println!("   To: {}", peer);
    println!("   Status: Processing... (check daemon logs)");
    
    Ok(())
}

fn find_daemon_binary() -> Result<PathBuf> {
    // Try to find the daemon binary in common locations
    let possible_paths = [
        "./target/release/landro-daemon",
        "./target/debug/landro-daemon", 
        "../target/release/landro-daemon",
        "../target/debug/landro-daemon",
        "target/release/landro-daemon",
        "target/debug/landro-daemon",
        "landro-daemon", // In PATH
    ];
    
    for path in &possible_paths {
        let path_buf = PathBuf::from(path);
        if path_buf.exists() || path == &"landro-daemon" {
            return Ok(path_buf);
        }
    }
    
    Err(anyhow::anyhow!(
        "Could not find landro-daemon binary. Please build it first with 'cargo build --release -p landro-daemon'"
    ))
}