//! Which provider served the merkle path behind each proof the toolbox's
//! monitor stored by its backstop (`check_for_proofs`,
//! `synchronize_transaction_statuses`).
//!
//! The toolbox (bsv-wallet-toolbox-rs 0.8.0) names the provider only at
//! debug (`services.rs`, "get_merkle_path: proof served": the provider and
//! the txid), and logs the stored proof at info without it
//! (`monitor/tasks/check_for_proofs.rs`, marker `proof_backstop_hit`: the
//! txid and the height). The providers it asks, in order, are Arcade (when
//! configured), then WhatsOnChain, then Bitails: the break-glass couriers
//! of rule 28 (bsv-stack-lean, the ruling of 2026-10-09; the toolbox's T6,
//! T7). This layer reads the two lines and says them as one, at info:
//!
//! ```text
//! INFO bsv_wallet_cli::backstop_log: the monitor's proof backstop stored a proof marker="proof_backstop_provider" provider=WhatsOnChain txid=… height=971555
//! ```
//!
//! A proof the triage's status answer carried (Arcade's MINED document,
//! marker `triage_inline_proof`) had no merkle path fetch; its provider
//! reads `status-answer`. Nothing is fetched here and nothing decided: the
//! proof was checked against the header service before it was stored.
//!
//! A layer cannot log through the dispatcher it is called from (the nested
//! event is dropped), so the line goes over a channel to a task that logs
//! it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Filter, Layer};

/// The toolbox's module that names the provider.
const SERVICES_TARGET: &str = "bsv_wallet_toolbox_rs::services::services";
/// The toolbox's monitor task that stores a proof by its backstop.
const BACKSTOP_TARGET: &str = "bsv_wallet_toolbox_rs::monitor::tasks::check_for_proofs";
const SERVED: &str = "get_merkle_path: proof served";
/// The txids whose provider is remembered at most (the tracker's own
/// re-asks are served through the same call and never reach the backstop).
const REMEMBERED: usize = 1024;

/// One proof the backstop stored, and who served its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackstopProof {
    /// The provider's name as the toolbox has it (`ArcadeV2`,
    /// `WhatsOnChain`, `Bitails`), or `status-answer`.
    pub provider: String,
    pub txid: String,
    /// The block height the toolbox stored, when it gave one.
    pub height: Option<u32>,
}

/// The layer: it remembers the provider of each served path and hands one
/// [`BackstopProof`] to `sink` per backstop hit.
pub struct BackstopLayer {
    served: Mutex<HashMap<String, String>>,
    sink: Box<dyn Fn(BackstopProof) + Send + Sync>,
}

impl BackstopLayer {
    pub fn new(sink: impl Fn(BackstopProof) + Send + Sync + 'static) -> Self {
        Self {
            served: Mutex::new(HashMap::new()),
            sink: Box::new(sink),
        }
    }

    /// The layer the binary installs: each hit is logged at info by a task
    /// on the running runtime.
    pub fn logging() -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BackstopProof>();
        tokio::spawn(async move {
            while let Some(hit) = rx.recv().await {
                log(&hit);
            }
        });
        Self::new(move |hit| {
            let _ = tx.send(hit);
        })
    }
}

/// The line.
pub fn log(hit: &BackstopProof) {
    tracing::info!(
        marker = "proof_backstop_provider",
        provider = %hit.provider,
        txid = %hit.txid,
        height = ?hit.height,
        "the monitor's proof backstop stored a proof"
    );
}

/// Only the two toolbox lines reach the layer, whatever the log level.
pub struct BackstopFilter;

impl<S> Filter<S> for BackstopFilter {
    fn enabled(&self, meta: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        meta.is_event()
            && ((meta.target() == SERVICES_TARGET && *meta.level() == Level::DEBUG)
                || (meta.target() == BACKSTOP_TARGET && *meta.level() == Level::INFO))
    }
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    marker: Option<String>,
    provider: Option<String>,
    txid: Option<String>,
    block_height: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.set(field, format!("{value:?}"));
    }
}

impl Fields {
    fn set(&mut self, field: &Field, value: String) {
        match field.name() {
            "message" => self.message = Some(value),
            "marker" => self.marker = Some(value),
            "provider" => self.provider = Some(value),
            "txid" => self.txid = Some(value),
            "block_height" => self.block_height = Some(value),
            _ => {}
        }
    }
}

/// `Some(971555)` (the toolbox logs the height as `?Option<u32>`), or a
/// bare number.
fn height_of(text: &str) -> Option<u32> {
    let digits = text.trim_start_matches("Some(").trim_end_matches(')');
    digits.parse().ok()
}

impl<S: Subscriber> Layer<S> for BackstopLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(txid) = fields.txid else {
            return;
        };
        let mut served = self.served.lock().unwrap_or_else(|e| e.into_inner());
        if fields.message.as_deref() == Some(SERVED) {
            if let Some(provider) = fields.provider {
                if served.len() >= REMEMBERED {
                    served.clear();
                }
                served.insert(txid, provider);
            }
        } else if fields.marker.as_deref() == Some("proof_backstop_hit") {
            let provider = served
                .remove(&txid)
                .unwrap_or_else(|| "status-answer".to_string());
            drop(served);
            (self.sink)(BackstopProof {
                provider,
                txid,
                height: fields.block_height.as_deref().and_then(height_of),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    /// The two toolbox lines, as the toolbox writes them, become one hit
    /// naming the provider, the txid and the height; a hit with no served
    /// path names the status answer.
    #[test]
    fn the_backstop_hit_names_the_provider_that_served_its_path() {
        let hits: Arc<Mutex<Vec<BackstopProof>>> = Arc::default();
        let sink = hits.clone();
        let layer = BackstopLayer::new(move |hit| sink.lock().unwrap().push(hit));
        let subscriber = tracing_subscriber::registry().with(layer.with_filter(BackstopFilter));
        let txid = "9b".repeat(32);
        let inline = "c4".repeat(32);
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(
                target: "bsv_wallet_toolbox_rs::services::services",
                provider = %"WhatsOnChain",
                txid = %txid,
                "get_merkle_path: proof served"
            );
            tracing::info!(
                target: "bsv_wallet_toolbox_rs::monitor::tasks::check_for_proofs",
                txid = %txid,
                status = ?"Completed",
                block_height = ?Some(971_555u32),
                marker = "proof_backstop_hit",
                "Transaction status synchronized by backstop fetch"
            );
            tracing::info!(
                target: "bsv_wallet_toolbox_rs::monitor::tasks::check_for_proofs",
                txid = %inline,
                block_height = ?Some(971_556u32),
                marker = "proof_backstop_hit",
                "Transaction status synchronized by backstop fetch"
            );
            // Another module's line with the same words is not read.
            tracing::debug!(
                target: "bsv_wallet_cli::elsewhere",
                provider = %"Bitails",
                txid = %inline,
                "get_merkle_path: proof served"
            );
        });
        let hits = hits.lock().unwrap();
        println!("{hits:?}");
        assert_eq!(
            *hits,
            vec![
                BackstopProof {
                    provider: "WhatsOnChain".into(),
                    txid,
                    height: Some(971_555),
                },
                BackstopProof {
                    provider: "status-answer".into(),
                    txid: inline,
                    height: Some(971_556),
                },
            ]
        );
    }
}
