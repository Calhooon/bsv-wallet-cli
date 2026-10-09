use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bsv-wallet", about = "Self-contained BSV wallet", version)]
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
        /// Hand the claim over before unlock. A broadcaster that takes it holds
        /// it until the unlock; one that calls it not final is not a failure:
        /// the claim is kept and announced again at the unlock time by the
        /// tracker's pass (`serve`, or `tracker-tick`)
        #[arg(long)]
        force: bool,
    },
    /// Send all spendable funds to one address (no change)
    Drain {
        /// Destination address
        address: String,
    },
    /// Internalize a BEEF transaction (receive funds): the routine way to receive, with the
    /// BEEF the payer hands over; checked against headers, no explorer asked
    Fund {
        /// BEEF transaction in hex (standard or AtomicBEEF)
        beef_hex: String,
        /// Output index to internalize (default: 0)
        #[arg(long, default_value_t = 0)]
        vout: u32,
    },
    /// Break-glass courier: fetch a transaction's BEEF by txid and internalize it
    ///
    /// For a payment to our bare address that came with no BEEF from its sender. The BEEF
    /// is fetched from a courier (WhatsOnChain's BEEF route, then the wallet's own
    /// services) and then checked like any other: its proofs must meet the header
    /// service's roots. The routine path is `fund` with the BEEF the payer hands over.
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
    /// Break-glass chain scan: ask an explorer for the unspent outputs at our deposit
    /// address and internalize new ones
    ///
    /// This is a chain scan (an address lookup at WhatsOnChain, one explorer), kept for
    /// one case: finding a payment nobody handed us. It is not the routine way to
    /// receive, and no daemon path runs it. The routine path is `fund` with the BEEF the
    /// payer hands over: that answer is checked against headers and asks no explorer.
    /// An empty list from the explorer means "it lists nothing", not "nothing was paid".
    Sync {
        /// Also RECONCILE: every DB-unspent output missing from the chain's
        /// unspent set is put to the spend probe (the one `cleanup-abandoned`
        /// and `reconcile-outputs` use), and an output spent by a transaction
        /// we hold a merkle proof of is relinquished. A spender not proven yet,
        /// or an answer the probe could not get, leaves the output alone (heals
        /// a restored-from-backup wallet whose stale rows otherwise produce
        /// double-spend inputs).
        #[arg(long)]
        reconcile_spent: bool,
    },
    /// Run all monitor tasks once and exit (one-shot equivalent of `daemon`). The
    /// header task's state and the proof gate persist in the wallet database, so
    /// the FIRST run queues the chain tip and the SECOND run (one that still sees
    /// that tip) processes it and opens the proof gate: no proof is stored before
    /// the second run, exactly as the daemon accepts nothing before a header has
    /// stayed the tip for a full cycle. Refused (non-zero exit) when
    /// CHAINTRACKS_URL=off: with no header service every proof is refused
    Tick,
    /// Run one pass of the transaction tracker and exit: each of this wallet's
    /// unproven transactions holds a word (built, announced, seen, mined); a
    /// broadcaster's word is a hint; a merkle proof is asked for one named
    /// transaction when a hint disagrees or its age is due (TRACKER_AGE_SECS,
    /// default 600), and only a proof checked against the header service
    /// writes `mined`. Asks no chain index. `serve` runs the same pass every
    /// 60 s. Refused (non-zero exit) when CHAINTRACKS_URL=off
    TrackerTick,
    /// Run monitor + HTTP server (foreground)
    Daemon {
        /// Bind beyond loopback (BIND_ADDR) with no AUTH_TOKEN set: without
        /// this flag such a bind is refused at startup
        #[arg(long)]
        allow_no_token: bool,
    },
    /// Run HTTP server only (no monitor)
    Serve {
        /// Bind beyond loopback (BIND_ADDR) with no AUTH_TOKEN set: without
        /// this flag such a bind is refused at startup
        #[arg(long)]
        allow_no_token: bool,
    },
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
        /// Bind beyond loopback (BIND_ADDR) with no AUTH_TOKEN set: without
        /// this flag such a bind is refused at startup
        #[arg(long)]
        allow_no_token: bool,
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
    /// Write a BEEF file for every unspent UTXO, built from the proofs and transactions
    /// the wallet already holds (no explorer is asked)
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
    /// Probe unproven txs for network presence (broadcaster, chain index), credit the
    /// broadcast memory, and retire phantom chains: a REJECTED tx, or one absent from
    /// every network source past BROADCAST_ABSENCE_MINUTES, together with every unproven
    /// descendant (inputs released only on chain verification)
    ReconcileBroadcasts {
        /// Apply changes (default is dry-run; probes run either way)
        #[arg(long)]
        execute: bool,
        /// Unproven transactions to probe this pass (run again to continue)
        #[arg(long, default_value_t = 200)]
        max_probes: usize,
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
    /// Re-prove stored merkle proofs the chain no longer confirms (after a reorg).
    /// Compares every stored proof's merkle root in the window with the canonical
    /// header's and lists the disagreements. Dry-run by default (no storage write).
    /// With --execute: a provider's validated proof for the canonical block replaces
    /// the stored one; providers still naming the stored block, or faulting, retain
    /// it (the monitor retries); only positive evidence (the tracker refutes the
    /// stored root, two providers answer cleanly "not mined", no provider serves a
    /// path) demotes it to unproven with its bytes kept. --execute first raises the
    /// persisted proof gate to tip - 1 when it is closed or below that, and is
    /// refused when CHAINTRACKS_URL=off (nothing can be refuted without a header
    /// service).
    Reproof {
        /// Only proofs at or above this height (default: the last 288 blocks). The
        /// window includes the un-aged tip: a replacement there is deferred by the
        /// proof gate and re-presented by the monitor, never stored early
        #[arg(long)]
        since_height: Option<u32>,
        /// Every stored proof, whatever its height: one chaintracks read per distinct
        /// height with no bound (progress every 50 heights on stderr)
        #[arg(long)]
        all: bool,
        /// Apply changes (default is dry-run)
        #[arg(long)]
        execute: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn help_of(name: &str) -> String {
        let mut cli = Cli::command();
        let sub = cli.find_subcommand_mut(name).expect("the subcommand");
        sub.render_long_help().to_string()
    }

    /// C4 (Rule 28): `sync --help` says what the command is, a chain scan
    /// and a break-glass read, and names the routine path. Red at the base:
    /// "Scan WhatsOnChain for unspent UTXOs at our deposit address and
    /// internalize new ones", and nothing else.
    #[test]
    fn sync_help_says_it_is_a_break_glass_chain_scan_and_names_fund() {
        let help = help_of("sync");
        for words in ["chain scan", "Break-glass", "`fund`", "BEEF"] {
            assert!(help.contains(words), "sync --help lacks `{words}`:\n{help}");
        }
    }
}
