use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bsv-wallet", about = "Self-contained BSV wallet")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Use testnet instead of mainnet
    #[arg(long, global = true)]
    pub testnet: bool,

    /// SQLite database path
    #[arg(long, global = true, default_value = "wallet.db")]
    pub db: String,

    /// HTTP server port
    #[arg(long, global = true, default_value_t = 3322)]
    pub port: u16,

    /// Output JSON instead of tables
    #[arg(long, global = true)]
    pub json: bool,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate/import identity, create wallet database
    Init {
        /// Import existing root key (hex)
        #[arg(long)]
        key: Option<String>,
        /// Overwrite existing .env (DESTROYS the existing wallet's key)
        #[arg(long)]
        force: bool,
    },
    /// Show public key and address
    Identity,
    /// Show spendable balance
    Balance,
    /// Show BRC-29 funding address
    Address,
    /// Send BSV to a P2PKH address
    Send {
        /// Destination address
        address: String,
        /// Amount in satoshis
        satoshis: u64,
    },
    /// Send all spendable funds to one address (no change)
    Drain {
        /// Destination address
        address: String,
    },
    /// Internalize a BEEF transaction (receive funds)
    Fund {
        /// BEEF transaction in hex (standard or AtomicBEEF)
        beef_hex: String,
        /// Output index to internalize (default: 0)
        #[arg(long, default_value_t = 0)]
        vout: u32,
    },
    /// Fetch a tx by txid from WhatsOnChain and internalize it
    Receive {
        /// Transaction id (hex)
        txid: String,
        /// Output index; if omitted, auto-selects the vout matching our deposit address
        #[arg(long)]
        vout: Option<u32>,
    },
    /// List unspent outputs
    Outputs {
        /// Filter by basket name
        #[arg(long)]
        basket: Option<String>,
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// List transaction history
    Actions {
        /// Filter by label
        #[arg(long)]
        label: Option<String>,
    },
    /// Scan WhatsOnChain for unspent UTXOs at our deposit address and internalize new ones
    Sync,
    /// Run monitor + HTTP server (foreground)
    Daemon,
    /// Run HTTP server only (no monitor)
    Serve,
    /// Split UTXOs into multiple outputs for concurrency
    Split {
        /// Number of output UTXOs to create
        #[arg(long, default_value_t = 3)]
        count: u32,
    },
    /// Show blockchain service status
    Services,
    /// Open database inspector UI in browser
    Ui {
        /// UI server port (default: 9321)
        #[arg(long, default_value_t = 9321)]
        ui_port: u16,
    },
    /// Compact stored BEEF blobs to reduce transaction proof sizes
    Compact,
    /// Bundle .env + wallet.db into a tar.gz for off-machine backup
    Backup {
        /// Output path (default: ./bsv-wallet-backup-<timestamp>.tar.gz)
        #[arg(long)]
        to: Option<std::path::PathBuf>,
    },
}
