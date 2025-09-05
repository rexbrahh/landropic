use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber;

#[derive(Parser)]
#[command(name = "landropic")]
#[command(about = "Cross-platform encrypted file sync", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Increase logging verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon
    Start {
        /// Bind address for QUIC server
        #[arg(short, long, default_value = "[::]:9876")]
        bind: String,
    },

    /// Stop the daemon
    Stop,

    /// Show daemon status
    Status,

    /// Initialize a new device identity
    Init {
        /// Device name
        #[arg(short, long)]
        name: String,
    },

    /// Pair with another device
    Pair {
        /// Pairing code or QR data
        code: String,
    },

    /// Add a folder to sync
    Add {
        /// Path to folder
        path: String,
    },

    /// List synced folders
    List,

    /// Show sync status
    Sync,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt().with_env_filter(log_level).init();

    match cli.command {
        Commands::Start { bind } => {
            println!("Starting Landropic daemon on {}", bind);
            // TODO: Start daemon
        }
        Commands::Stop => {
            println!("Stopping Landropic daemon");
            // TODO: Stop daemon
        }
        Commands::Status => {
            println!("Landropic daemon status");
            // TODO: Show status
        }
        Commands::Init { name } => {
            println!("Initializing device identity: {}", name);

            use landro_crypto::DeviceIdentity;
            let mut identity = DeviceIdentity::generate(&name)?;
            identity.save(None).await?;

            println!("Device ID: {}", identity.device_id());
            println!("Identity saved to ~/.landropic/keys/device_identity.key");
        }
        Commands::Pair { code } => {
            println!("Pairing with device using code: {}", code);
            // TODO: Implement pairing
        }
        Commands::Add { path } => {
            println!("Adding folder to sync: {}", path);
            // TODO: Add folder
        }
        Commands::List => {
            println!("Synced folders:");
            // TODO: List folders
        }
        Commands::Sync => {
            println!("Sync status:");
            // TODO: Show sync status
        }
    }

    Ok(())
}
