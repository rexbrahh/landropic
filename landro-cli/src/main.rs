use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio;

mod config;
mod daemon_client;

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
    Unsync { folder: PathBuf },

    /// List paired devices
    Peers {
        #[arg(long)]
        json: bool,
    },

    /// Manage background daemon
    #[command(subcommand)]
    Daemon(DaemonCommands),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand)]
enum DaemonCommands {
    Start,
    Stop,
    Restart,
    Status,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show {
        #[arg(long)]
        json: bool,
    },
    
    /// Set a configuration value
    Set {
        /// Configuration key (device_name, daemon_port, storage_path)
        key: String,
        /// Value to set
        value: String,
    },
    
    /// Get a specific configuration value
    Get {
        /// Configuration key to retrieve
        key: String,
    },
    
    /// Reset configuration to defaults
    Reset {
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    use colored::Colorize;

    let cli = Cli::parse();

    // Setup logging
    setup_logging(cli.verbose);

    // Handle commands with better error reporting
    let result = match cli.command {
        Commands::Init { name } => handle_init(name).await,
        Commands::Pair { show_qr, code } => handle_pair(show_qr, code).await,
        Commands::Sync { folder, watch } => handle_sync(folder, watch).await,
        Commands::Status { json, folder } => handle_status(json, folder).await,
        Commands::List { json } => handle_list(json).await,
        Commands::Unsync { folder } => handle_unsync(folder).await,
        Commands::Peers { json } => handle_peers(json).await,
        Commands::Daemon(cmd) => handle_daemon(cmd).await,
        Commands::Config(cmd) => handle_config(cmd).await,
    };

    // Handle errors with colored output
    if let Err(error) = result {
        eprintln!("{} {}", "Error:".red().bold(), error);

        // Provide helpful hints for common errors
        let error_str = error.to_string().to_lowercase();
        if error_str.contains("connection refused") || error_str.contains("daemon") {
            eprintln!("{} Try: landropic daemon start", "Hint:".yellow());
        } else if error_str.contains("not found") {
            eprintln!(
                "{} Check if the path exists and is accessible",
                "Hint:".yellow()
            );
        } else if error_str.contains("permission") {
            eprintln!("{} Check file permissions", "Hint:".yellow());
        }

        std::process::exit(1);
    }

    Ok(())
}

async fn handle_init(name: Option<String>) -> Result<()> {
    use colored::Colorize;

    // Check if already initialized
    let config_path = config::get_config_path()?;
    if config_path.exists() {
        println!(
            "{} {}",
            "✓".green(),
            "Landropic is already initialized on this device".bold()
        );
        return Ok(());
    }

    // Generate device identity
    let device_name = name.unwrap_or_else(|| whoami::devicename());

    println!("{} Initializing Landropic...", "●".blue());

    let mut identity = landro_crypto::DeviceIdentity::generate(&device_name)?;
    identity.save(None).await?;

    // Create config
    let config = config::Config {
        device_name: device_name.clone(),
        device_id: identity.device_id().to_string(),
        daemon_port: 7703,
        storage_path: config::default_storage_path()?,
    };
    config.save()?;

    // Create directories
    std::fs::create_dir_all(&config.storage_path)?;

    // Start daemon
    daemon_client::start_daemon().await?;

    println!(
        "{} Landropic initialized for device: {}",
        "✓".green(),
        device_name.bold()
    );
    println!(
        "{} Device ID: {}",
        "✓".green(),
        format!("{}", identity.device_id()).dimmed()
    );
    println!("{} Background sync daemon started", "✓".green());
    println!(
        "\n{} Use 'landropic pair --show-qr' to pair with another device",
        "💡".blue()
    );

    Ok(())
}

async fn handle_pair(show_qr: bool, code: Option<String>) -> Result<()> {
    use colored::Colorize;

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    if show_qr {
        let pairing_info = client.get_pairing_info().await?;

        // Generate QR code
        println!("\n{} Show this QR code to the other device:", "📱".blue());
        qr2term::print_qr(&pairing_info)
            .map_err(|e| anyhow::anyhow!("Failed to generate QR code: {}", e))?;
        println!(
            "\nOr enter this code manually: {}",
            pairing_info.bold().yellow()
        );

        // Wait for pairing
        println!("\n{} Waiting for pairing...", "⏳".blue());
        client.wait_for_pairing().await?;
        println!("{} Successfully paired!", "✓".green());
    } else if let Some(code) = code {
        println!("{} Pairing with device...", "●".blue());
        client.pair_with_code(&code).await?;
        println!("{} Successfully paired with device", "✓".green());
    } else {
        println!("Device Pairing");
        println!("{}  Use --show-qr to display pairing code", "💡".blue());
        println!("{}  Use --code <CODE> to enter a pairing code", "💡".blue());
    }

    Ok(())
}

async fn handle_sync(folder: PathBuf, watch: bool) -> Result<()> {
    use colored::Colorize;
    use indicatif::{ProgressBar, ProgressStyle};

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    let folder = folder
        .canonicalize()
        .context("Failed to resolve folder path")?;

    client.add_sync_folder(&folder, watch).await?;

    if watch {
        println!(
            "✓ {}: {}",
            "Watching folder for changes".green(),
            folder.display()
        );
        return Ok(());
    }

    // Show progress for one-time syncs
    println!("{} {}", "Starting sync for:".blue(), folder.display());

    let pb = ProgressBar::new(100);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40}] {percent}% {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    pb.set_message("Scanning files...");

    // Poll for sync progress
    let mut last_progress = 0u64;
    let mut attempts = 0;
    const MAX_ATTEMPTS: u32 = 300; // 5 minutes at 1 second intervals

    loop {
        if attempts >= MAX_ATTEMPTS {
            pb.finish_with_message("Sync timeout - check daemon status");
            return Err(anyhow::anyhow!("Sync timeout after 5 minutes"));
        }

        match client.get_sync_progress(Some(&folder)).await {
            Ok(progress) => {
                let progress_percent = if progress.total_files > 0 {
                    (progress.files_completed * 100) / progress.total_files
                } else {
                    0
                };

                pb.set_position(progress_percent);

                let msg = match progress.phase.as_str() {
                    "scanning" => "Scanning files...".to_string(),
                    "uploading" => format!(
                        "Uploading ({}/{})",
                        progress.files_completed, progress.total_files
                    ),
                    "downloading" => format!(
                        "Downloading ({}/{})",
                        progress.files_completed, progress.total_files
                    ),
                    "complete" => "Complete!".to_string(),
                    _ => format!(
                        "Syncing ({}/{})",
                        progress.files_completed, progress.total_files
                    ),
                };

                if let Some(current_file) = &progress.current_file {
                    let filename = std::path::Path::new(current_file)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(current_file);
                    pb.set_message(format!("{} - {}", msg, filename));
                } else {
                    pb.set_message(msg);
                }

                if progress.phase == "complete" {
                    pb.finish_with_message("✓ Sync completed successfully");
                    break;
                }

                last_progress = progress.files_completed;
            }
            Err(_) => {
                // Daemon might not have the endpoint yet, or sync might be done
                pb.set_message("Checking status...");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        attempts += 1;
    }

    Ok(())
}

async fn handle_status(json: bool, folder: Option<PathBuf>) -> Result<()> {
    use colored::Colorize;

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    let status = client.get_status(folder).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{}", "Landropic Status".bold().blue());
        println!("{}", "================".blue());
        println!("Device: {}", status.device_name.bold());
        println!(
            "Status: {}",
            if status.daemon_status == "running" {
                status.daemon_status.green()
            } else {
                status.daemon_status.red()
            }
        );

        let peers_text = format!("{} connected", status.connected_peers);
        println!(
            "Peers: {}",
            if status.connected_peers > 0 {
                peers_text.green()
            } else {
                peers_text.yellow()
            }
        );

        if !status.sync_folders.is_empty() {
            println!("\n{}:", "Synced Folders".bold());
            for folder in status.sync_folders {
                let status_color = match folder.status.as_str() {
                    "synced" => folder.status.green(),
                    "syncing" => folder.status.blue(),
                    "error" => folder.status.red(),
                    _ => folder.status.yellow(),
                };
                println!("  {} {} ({})", "•".cyan(), folder.path, status_color);

                if folder.files_synced > 0 || folder.files_pending > 0 {
                    let synced_text = format!("{} synced", folder.files_synced).green();
                    let pending_text = if folder.files_pending > 0 {
                        format!(", {} pending", folder.files_pending)
                            .yellow()
                            .to_string()
                    } else {
                        "".to_string()
                    };
                    println!("    {}{}", synced_text, pending_text);
                }
            }
        } else {
            println!("\n{}", "No folders being synced".yellow());
            println!("Use 'landropic sync <folder>' to start syncing");
        }
    }

    Ok(())
}

async fn handle_list(json: bool) -> Result<()> {
    use colored::Colorize;

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    let folders = client.list_folders().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&folders)?);
    } else {
        if folders.is_empty() {
            println!("{}", "No folders are being synced".yellow());
            println!("Use 'landropic sync <folder>' to start syncing a folder");
        } else {
            println!("{}:", "Synced Folders".bold().blue());
            for folder in folders {
                println!("  {} {}", "•".cyan(), folder);
            }
        }
    }

    Ok(())
}

async fn handle_unsync(folder: PathBuf) -> Result<()> {
    use colored::Colorize;

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    let folder = folder
        .canonicalize()
        .context("Failed to resolve folder path")?;

    client.remove_sync_folder(&folder).await?;
    println!("{} Stopped syncing: {}", "✓".green(), folder.display());

    Ok(())
}

async fn handle_peers(json: bool) -> Result<()> {
    use colored::Colorize;

    ensure_daemon_running().await?;
    let client = daemon_client::Client::new()?;

    let peers = client.get_peers().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&peers)?);
    } else {
        if peers.is_empty() {
            println!("{}", "No paired devices found".yellow());
            println!("Use 'landropic pair --show-qr' to pair with another device");
        } else {
            println!("Paired Devices:");
            for peer in peers {
                let status_indicator = if peer.connected {
                    "●".green()
                } else {
                    "○".yellow()
                };

                let status_text = if peer.connected {
                    "connected".green()
                } else {
                    "offline".yellow()
                };

                println!(
                    "  {} {} {} ({})",
                    status_indicator,
                    peer.name.bold(),
                    status_text,
                    peer.device_id.dimmed()
                );

                if let Some(last_seen) = peer.last_seen {
                    if !peer.connected {
                        println!("    Last seen: {}", last_seen.dimmed());
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_daemon(cmd: DaemonCommands) -> Result<()> {
    use colored::Colorize;

    match cmd {
        DaemonCommands::Start => {
            if daemon_client::is_running().await {
                println!("{} Daemon is already running", "ℹ".blue());
            } else {
                println!("{} Starting daemon...", "●".blue());
                daemon_client::start_daemon().await?;
                println!("{} Daemon started", "✓".green());
            }
        }
        DaemonCommands::Stop => {
            if daemon_client::is_running().await {
                println!("{} Stopping daemon...", "●".blue());
                daemon_client::stop_daemon().await?;
                println!("{} Daemon stopped", "✓".green());
            } else {
                println!("{} Daemon is not running", "ℹ".yellow());
            }
        }
        DaemonCommands::Restart => {
            println!("{} Restarting daemon...", "●".blue());
            daemon_client::stop_daemon().await?;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            daemon_client::start_daemon().await?;
            println!("{} Daemon restarted", "✓".green());
        }
        DaemonCommands::Status => {
            if daemon_client::is_running().await {
                println!("{} Daemon is running", "✓".green());
            } else {
                println!("{} Daemon is not running", "✗".red());
                println!("{} Use 'landropic daemon start' to start it", "💡".blue());
            }
        }
    }
    Ok(())
}

async fn handle_config(cmd: ConfigCommands) -> Result<()> {
    use colored::Colorize;

    match cmd {
        ConfigCommands::Show { json } => {
            let config = config::Config::load()?;
            
            if json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                println!("{}", "Landropic Configuration".bold().blue());
                println!("{}", "======================".blue());
                println!("Device Name: {}", config.device_name.bold());
                println!("Device ID: {}", config.device_id.dimmed());
                println!("Daemon Port: {}", config.daemon_port.to_string().cyan());
                println!("Storage Path: {}", config.storage_path.display().to_string().green());
            }
        }
        ConfigCommands::Get { key } => {
            let config = config::Config::load()?;
            
            match key.to_lowercase().as_str() {
                "device_name" | "device-name" => println!("{}", config.device_name),
                "device_id" | "device-id" => println!("{}", config.device_id),
                "daemon_port" | "daemon-port" => println!("{}", config.daemon_port),
                "storage_path" | "storage-path" => println!("{}", config.storage_path.display()),
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unknown configuration key: {}. Valid keys: device_name, device_id, daemon_port, storage_path",
                        key
                    ));
                }
            }
        }
        ConfigCommands::Set { key, value } => {
            let mut config = config::Config::load()?;
            
            match key.to_lowercase().as_str() {
                "device_name" | "device-name" => {
                    config.device_name = value.clone();
                    println!("{} Set device_name to: {}", "✓".green(), value.bold());
                }
                "daemon_port" | "daemon-port" => {
                    let port: u16 = value.parse()
                        .context("Invalid port number. Must be between 1 and 65535")?;
                    config.daemon_port = port;
                    println!("{} Set daemon_port to: {}", "✓".green(), port.to_string().cyan());
                }
                "storage_path" | "storage-path" => {
                    let path = PathBuf::from(&value);
                    // Verify the path is valid
                    if !path.parent().map(|p| p.exists()).unwrap_or(false) {
                        return Err(anyhow::anyhow!("Parent directory does not exist: {}", value));
                    }
                    config.storage_path = path;
                    println!("{} Set storage_path to: {}", "✓".green(), value.green());
                }
                "device_id" | "device-id" => {
                    return Err(anyhow::anyhow!(
                        "Cannot modify device_id. This is generated during initialization"
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unknown configuration key: {}. Valid keys: device_name, daemon_port, storage_path",
                        key
                    ));
                }
            }
            
            config.save()?;
            println!("\n{} Restart the daemon for changes to take effect", "💡".blue());
        }
        ConfigCommands::Reset { force } => {
            if !force {
                println!("{}", "This will reset all configuration to default values.".yellow());
                println!("Use --force to confirm");
                return Ok(());
            }
            
            let device_name = whoami::devicename();
            let config = config::Config {
                device_name: device_name.clone(),
                device_id: String::new(), // Will need to regenerate identity
                daemon_port: 7703,
                storage_path: config::default_storage_path()?,
            };
            
            config.save()?;
            println!("{} Configuration reset to defaults", "✓".green());
            println!("{} You will need to re-initialize with 'landropic init'", "⚠".yellow());
        }
    }
    
    Ok(())
}

async fn ensure_daemon_running() -> Result<()> {
    use colored::Colorize;

    if !daemon_client::is_running().await {
        println!("{} Starting landropic daemon...", "●".blue());
        daemon_client::start_daemon().await.context(format!(
            "{} Failed to start daemon. Try 'landropic daemon start' manually.",
            "Error:".red()
        ))?;
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
