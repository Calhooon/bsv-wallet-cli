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
    /// Send a time-locked BSV gift to a recipient's public key
    GiftSend {
        /// Recipient's 33-byte compressed public key (hex)
        recipient: String,
        /// Gift amount in satoshis
        satoshis: u64,
        /// Unlock time: unix timestamp, YYYY-MM-DD, or RFC-3339
        #[arg(long)]
        unlock: String,
        /// Recipient-owned fee UTXO (sats) bundled so the claim self-funds its fee
        #[arg(long, default_value_t = 1000)]
        fee_utxo: u64,
    },
    /// Inspect a time-locked gift (prove it's yours + when it unlocks; no claim)
    GiftInspect {
        /// The gift (deposit) transaction id
        txid: String,
    },
    /// Claim a time-locked gift after its unlock time
    GiftClaim {
        /// The gift (deposit) transaction id
        txid: String,
        /// Pre-broadcast before unlock (sits unconfirmed, auto-confirms at unlock)
        #[arg(long)]
        force: bool,
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
    Sync {
        /// Also RECONCILE: for every DB-unspent output missing from the
        /// chain's unspent set, per-outpoint spent-check WoC and relinquish
        /// outputs the chain says are SPENT (heals a restored-from-backup
        /// wallet whose stale rows otherwise produce double-spend inputs).
        #[arg(long)]
        reconcile_spent: bool,
    },
    /// Run all monitor tasks once and exit (one-shot equivalent of `daemon`)
    Tick,
    /// Run monitor + HTTP server (foreground)
    Daemon,
    /// Run HTTP server only (no monitor)
    Serve,
    /// ONE process serving MANY wallets (fleet mode): each --wallet
    /// <seat-dir>:<port> serves <seat-dir>/wallet.db on its own port with the
    /// ROOT_KEY read from <seat-dir>/.env (never process env). Per-seat port
    /// contract unchanged; any tenant exiting ends the whole process.
    ServeFleet {
        /// Repeated: <seat-dir>:<port> (dir holds wallet.db + .env)
        #[arg(long = "wallet", required = true)]
        wallet: Vec<String>,
        /// Run each tenant's FULL daemon (monitor + auto-reconcile), not bare
        /// HTTP — what a fleet playing back-to-back hands needs (bare serve
        /// never proves 0-conf ancestry; the createAction-502 class returns).
        #[arg(long)]
        daemon: bool,
    },
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
    /// Fetch BEEFs from WhatsOnChain for every unspent UTXO and write them as files
    ExportBeefs {
        /// Output directory (will be created if missing)
        #[arg(long)]
        to: std::path::PathBuf,
    },
    /// Find unproven txs that aren't on chain, mark failed, restore their inputs
    CleanupAbandoned {
        /// Apply changes (default is dry-run)
        #[arg(long)]
        execute: bool,
    },
    /// Re-mark spent every output a LIVE tx of this wallet already spends (DB-only), then
    /// chain-check the remaining spendable outputs per outpoint: relinquish when a confirmed
    /// tx spent them, keep untouched when unknown (an unknown never moves money)
    ReconcileOutputs {
        /// Apply changes (default is dry-run)
        #[arg(long)]
        execute: bool,
        /// Spendable outputs to chain-check per pass (run again to continue)
        #[arg(long, default_value_t = 200)]
        max_chain_checks: usize,
    },
}
