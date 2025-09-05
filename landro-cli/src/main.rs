use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio;

mod daemon_client;
mod config;

#[derive(Parser)]
#[command(name = "landropic")]
#[command(about = "Cross-platform encrypted file sync", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize landropic on this device
    Init {
        #[arg(short, long)]
        name: Option<String>,
    },
    
    /// Pair with another device  
    Pair {
        #[arg(long, conflicts_with = "code")]
        show_qr: bool,
        
        #[arg(long, conflicts_with = "show_qr")]
        code: Option<String>,
    },
    
    /// Start syncing a folder
    Sync {
        folder: PathBuf,
        
        #[arg(short, long)]
        watch: bool,
    },
    
    /// Show sync status
    Status {
        #[arg(long)]
        json: bool,
        
        #[arg(short, long)]
        folder: Option<PathBuf>,
    },
    
    /// List synced folders
    List {
        #[arg(long)]
        json: bool,
    },
    
    /// Stop syncing a folder
    Unsync {
        folder: PathBuf,
    },
    
    /// Manage background daemon
    #[command(subcommand)]
    Daemon(DaemonCommands),
}

#[derive(Subcommand)]
enum DaemonCommands {
    Start,
    Stop, 
    Restart,
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Setup logging
    setup_logging(cli.verbose);
    
    // Handle commands
    match cli.command {
        Commands::Init { name } => handle_init(name).await,
        Commands::Pair { show_qr, code } => handle_pair(show_qr, code).await,
        Commands::Sync { folder, watch } => handle_sync(folder, watch).await,
        Commands::Status { json, folder } => handle_status(json, folder).await,
        Commands::List { json } => handle_list(json).await,
        Commands::Unsync { folder } => handle_unsync(folder).await,
        Commands::Daemon(cmd) => handle_daemon(cmd).await,
    }
}

async fn handle_init(name: Option<String>) -> Result<()> {
    // Check if already initialized
    let config_path = config::get_config_path()?;
    if config_path.exists() {
        println!("Landropic is already initialized on this device");
        return Ok(());
    }
    
    // Generate device identity
    let device_name = name.unwrap_or_else(|| {
        whoami::devicename()
    });
    
    let mut identity = landro_crypto::DeviceIdentity::generate(&device_name)?;
    identity.save(None).await?;
    
    // Create config
    let config = config::Config {
        device_name: device_name.clone(),
        device_id: identity.device_id().to_string(),
        daemon_port: 7890,
        storage_path: config::default_storage_path()?,
    };
    config.save()?;
    
    // Create directories
    std::fs::create_dir_all(&config.storage_path)?;
    
    // Start daemon
    daemon_client::start_daemon().await?;
    
    println!("✓ Landropic initialized for device: {}", device_name);
    println!("✓ Device ID: {}", identity.device_id());
    println!("✓ Background sync daemon started");
    
    Ok(())
}

async fn handle_pair(show_qr: bool, code: Option<String>) -> Result<()> {
    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;
    
    if show_qr {
        let pairing_info = client.get_pairing_info().await?;
        
        // Generate QR code
        println!("Show this QR code on the other device:");
        qr2term::print_qr(&pairing_info)
            .map_err(|e| anyhow::anyhow!("Failed to generate QR code: {}", e))?;
        println!("\nOr enter this code manually: {}", pairing_info);
        
        // Wait for pairing
        println!("Waiting for pairing...");
        client.wait_for_pairing().await?;
        println!("✓ Successfully paired!");
        
    } else if let Some(code) = code {
        client.pair_with_code(&code).await?;
        println!("✓ Successfully paired with device");
    } else {
        println!("Use --show-qr to display pairing code or --code to enter one");
    }
    
    Ok(())
}

async fn handle_sync(folder: PathBuf, watch: bool) -> Result<()> {
    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;
    
    let folder = folder.canonicalize()
        .context("Failed to resolve folder path")?;
    
    client.add_sync_folder(&folder, watch).await?;
    
    if watch {
        println!("✓ Watching folder for changes: {}", folder.display());
    } else {
        println!("✓ One-time sync started for: {}", folder.display());
    }
    
    Ok(())
}

async fn handle_status(json: bool, folder: Option<PathBuf>) -> Result<()> {
    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;
    
    let status = client.get_status(folder).await?;
    
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("Landropic Status");
        println!("================");
        println!("Device: {}", status.device_name);
        println!("Status: {}", status.daemon_status);
        println!("Peers: {} connected", status.connected_peers);
        
        if !status.sync_folders.is_empty() {
            println!("\nSynced Folders:");
            for folder in status.sync_folders {
                println!("  • {} ({})", folder.path, folder.status);
                if folder.files_synced > 0 {
                    println!("    {} files synced, {} pending", 
                        folder.files_synced, folder.files_pending);
                }
            }
        }
    }
    
    Ok(())
}

async fn handle_list(json: bool) -> Result<()> {
    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;
    
    let folders = client.list_folders().await?;
    
    if json {
        println!("{}", serde_json::to_string_pretty(&folders)?);
    } else {
        if folders.is_empty() {
            println!("No folders are being synced");
        } else {
            println!("Synced Folders:");
            for folder in folders {
                println!("  • {}", folder);
            }
        }
    }
    
    Ok(())
}

async fn handle_unsync(folder: PathBuf) -> Result<()> {
    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;
    
    let folder = folder.canonicalize()
        .context("Failed to resolve folder path")?;
    
    client.remove_sync_folder(&folder).await?;
    println!("✓ Stopped syncing: {}", folder.display());
    
    Ok(())
}

async fn handle_daemon(cmd: DaemonCommands) -> Result<()> {
    match cmd {
        DaemonCommands::Start => {
            daemon_client::start_daemon().await?;
            println!("✓ Daemon started");
        }
        DaemonCommands::Stop => {
            daemon_client::stop_daemon().await?;
            println!("✓ Daemon stopped");
        }
        DaemonCommands::Restart => {
            daemon_client::stop_daemon().await?;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            daemon_client::start_daemon().await?;
            println!("✓ Daemon restarted");
        }
        DaemonCommands::Status => {
            if daemon_client::is_running().await {
                println!("✓ Daemon is running");
            } else {
                println!("✗ Daemon is not running");
            }
        }
    }
    Ok(())
}

async fn ensure_daemon_running() -> Result<()> {
    if !daemon_client::is_running().await {
        println!("Starting landropic daemon...");
        daemon_client::start_daemon().await?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

fn setup_logging(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    
    let filter = match verbosity {
        0 => EnvFilter::new("landropic=info"),
        1 => EnvFilter::new("landropic=debug"),
        _ => EnvFilter::new("landropic=trace"),
    };
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
