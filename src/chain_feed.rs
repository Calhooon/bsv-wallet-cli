//! The header service's chain-event feed, held by the daemon.
//!
//! The header service (`CHAINTRACKS_URL`, required, no default: P0-1c)
//! journals every tip move, fork, reorg, invalidation, freeze, tip age and
//! rung split as an envelope with a cursor (chaintracks-cloudflare v0.4.1
//! `docs/CHAIN-EVENTS.md`, "Delivery and cursors"). The daemon holds one
//! loop over it:
//!
//! 1. On start, replay from the stored cursor (`0` at birth) through the
//!    page endpoint, `GET /events?since=<cursor>`, until `hasMore` is false.
//! 2. Then hold the stream, `GET /events/stream?since=<cursor>` (SSE: an
//!    `id:` per envelope, comment heartbeats).
//! 3. Every envelope goes through `bsv_tracker::decode_envelope` and
//!    `Tracker::on_chain` with the new branch's headers: the header
//!    service's answer, except a height where it still gives a header the
//!    reorg deactivated ([`tracker_host::deliver_chain_event`]; bsv-tracker
//!    0.7.0, ruling (5) of 2026-10-20); the cursor is stored after
//!    each delivery, in the same transaction as the rows it moved, in the
//!    database the tracker's rows live in (`tracker_meta`, key
//!    `feed_cursor`: one row, one column).
//! 4. A dropped stream (EOF without an error event) reconnects: the page
//!    from the stored cursor, then the stream with `Last-Event-ID`.
//! 5. A decode fault, a cursor ahead of the journal (HTTP 409
//!    `cursorAhead`), an `event: error` (`feedUnavailable`) or a refused
//!    request (400, 404) is reported through the daemon's status
//!    ([`FeedStatus::fault`]: its kind, the cursor the loop stopped at, the
//!    offending envelope's cursor, the fault text) and the loop stops there:
//!    no skip, no acknowledgment, no silent reconnect past it (the tracker's
//!    host rule, bsv-tracker 0.4.0 `docs/CHAIN-EVENTS.md`). A restart of the
//!    daemon starts again from the stored cursor.
//!
//! # Load
//!
//! One outbound stream per daemon, plus one page request per (re)connect
//! (more only while the journal answers `hasMore`). The daemon runs nothing
//! periodic of its own for this: between envelopes it waits on the stream,
//! and the header service sends the heartbeats. A transport failure (no
//! answer, a 5xx, a 429) is retried after a pause that doubles from one
//! second to five minutes. The header service is read beyond the feed only
//! when an envelope names a tracked transaction: its tip and the heights of
//! the named transactions' proofs. With the feed held, the daemon's pass
//! keeps no ring of tips and rereads no mined row: a proof is fetched only
//! for a word whose root the event's recheck found gone.

use std::time::Duration;

use anyhow::Result;
use bsv_tracker::{decode_envelope, Clock};
use bsv_wallet_toolbox::{StorageSqlx, WalletServices};
use serde::{Deserialize, Serialize};

use crate::tracker_host::{self, TickOptions};

/// The pauses of a reconnect after a transport failure.
#[derive(Debug, Clone)]
pub struct FeedOptions {
    /// The first pause, and the pause after a stream that delivered.
    pub min_pause: Duration,
    /// The pause doubles up to this.
    pub max_pause: Duration,
}

impl Default for FeedOptions {
    fn default() -> Self {
        Self {
            min_pause: Duration::from_secs(1),
            max_pause: Duration::from_secs(300),
        }
    }
}

/// Why the loop stopped. The loop never moves past it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedFault {
    /// `decode` (an envelope or page the decoder refused), `cursorAhead`
    /// (HTTP 409), `feedUnavailable` (an SSE `event: error`), or `refused`
    /// (HTTP 400 or 404: a malformed cursor, or no feed at this source).
    pub kind: String,
    /// The stored cursor: the last envelope delivered, where the loop
    /// stopped.
    pub cursor: String,
    /// The offending envelope's cursor, when it carried one.
    pub event_cursor: Option<String>,
    /// The fault, as the decoder or the header service gave it.
    pub detail: String,
}

/// The daemon's view of its feed, stored in `tracker_meta` (key
/// `feed_status`) and served at `GET /chain-feed`. Rewritten at every
/// change of state and after every delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedStatus {
    /// The header service (`CHAINTRACKS_URL`).
    pub source: String,
    /// `replaying` (pages), `following` (the stream), `reconnecting`
    /// (after a transport failure or a dropped stream) or `stopped`.
    pub state: String,
    /// The stored cursor.
    pub cursor: String,
    /// Envelopes delivered since the daemon started.
    pub delivered: u64,
    /// Stream connections after the first.
    pub reconnects: u64,
    /// Set when the loop stopped: what it stopped on.
    pub fault: Option<FeedFault>,
    /// The last transport failure, retried.
    pub last_error: Option<String>,
    /// Seconds since the epoch, at the last change or delivery.
    pub updated_at: u64,
}

/// The status the feed stored last, if it ever ran here.
pub async fn read_status(storage: &StorageSqlx) -> Result<Option<FeedStatus>> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM tracker_meta WHERE key = 'feed_status'")
            .fetch_optional(storage.pool())
            .await
            .unwrap_or(None);
    Ok(match value {
        Some(v) => Some(serde_json::from_str(&v)?),
        None => None,
    })
}

async fn write_status(storage: &StorageSqlx, status: &FeedStatus) {
    let written = sqlx::query(
        "INSERT INTO tracker_meta (key, value) VALUES ('feed_status', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(serde_json::to_string(status).unwrap_or_default());
    let written = match crate::store::writer(storage).await {
        Ok(mut writer) => written
            .execute(&mut *writer)
            .await
            .map_err(anyhow::Error::from),
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        tracing::warn!(error = %e, "chain feed: the status was not stored");
    }
}

/// How one leg (the page replay, or one stream connection) ended.
enum Leg {
    /// The stream ended without an error event.
    Dropped { delivered: bool },
    /// Stop here and report.
    Stop(FeedFault),
    /// Retry after a pause.
    Transient(String),
}

struct Feed<'a, V: ?Sized, C> {
    base: String,
    client: reqwest::Client,
    storage: &'a StorageSqlx,
    services: &'a V,
    clock: &'a C,
    opts: &'a TickOptions,
    status: FeedStatus,
}

/// Hold the feed at `base` until it stops on a fault; return the status it
/// stopped with (also stored). See the module docs.
pub async fn run<V: WalletServices + ?Sized, C: Clock>(
    base: &str,
    storage: &StorageSqlx,
    services: &V,
    clock: &C,
    opts: &TickOptions,
    feed_opts: &FeedOptions,
) -> FeedStatus {
    let base = base.trim_end_matches('/').to_string();
    let cursor = match tracker_host::feed_cursor(storage).await {
        Ok(c) => c,
        Err(e) => {
            let status = FeedStatus {
                source: base,
                state: "stopped".into(),
                cursor: "?".into(),
                delivered: 0,
                reconnects: 0,
                fault: Some(FeedFault {
                    kind: "decode".into(),
                    cursor: "?".into(),
                    event_cursor: None,
                    detail: format!("the stored cursor: {e}"),
                }),
                last_error: None,
                updated_at: clock.now(),
            };
            write_status(storage, &status).await;
            return status;
        }
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let mut feed = Feed {
        status: FeedStatus {
            source: base.clone(),
            state: "replaying".into(),
            cursor: cursor.to_string(),
            delivered: 0,
            reconnects: 0,
            fault: None,
            last_error: None,
            updated_at: clock.now(),
        },
        base,
        client,
        storage,
        services,
        clock,
        opts,
    };
    let mut pause = feed_opts.min_pause;
    let mut connects: u64 = 0;
    loop {
        feed.set_state("replaying").await;
        let leg = match feed.replay().await {
            Ok(()) => {
                feed.set_state("following").await;
                let leg = feed.stream(connects > 0).await;
                connects += 1;
                feed.status.reconnects = connects - 1;
                leg
            }
            Err(leg) => leg,
        };
        match leg {
            Leg::Stop(fault) => return feed.stop(fault).await,
            Leg::Dropped { delivered } => {
                if delivered {
                    pause = feed_opts.min_pause;
                }
                feed.status.last_error = Some("the stream ended without an error event".into());
                tracing::info!(
                    cursor = %feed.status.cursor,
                    "chain feed: the stream dropped; reconnecting from the stored cursor"
                );
            }
            Leg::Transient(e) => {
                tracing::warn!(cursor = %feed.status.cursor, error = %e, "chain feed: retrying");
                feed.status.last_error = Some(e);
            }
        }
        feed.set_state("reconnecting").await;
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(feed_opts.max_pause);
    }
}

impl<V: WalletServices + ?Sized, C: Clock> Feed<'_, V, C> {
    async fn set_state(&mut self, state: &str) {
        self.status.state = state.into();
        self.status.updated_at = self.clock.now();
        write_status(self.storage, &self.status).await;
    }

    async fn stop(mut self, fault: FeedFault) -> FeedStatus {
        tracing::error!(
            kind = %fault.kind,
            cursor = %fault.cursor,
            event_cursor = ?fault.event_cursor,
            detail = %fault.detail,
            "chain feed stopped at its cursor; nothing past it is delivered until the daemon restarts"
        );
        self.status.fault = Some(fault);
        self.set_state("stopped").await;
        self.status
    }

    fn fault(&self, kind: &str, event_cursor: Option<&str>, detail: impl Into<String>) -> Leg {
        Leg::Stop(FeedFault {
            kind: kind.into(),
            cursor: self.status.cursor.clone(),
            event_cursor: event_cursor.map(str::to_string),
            detail: detail.into(),
        })
    }

    /// Read an HTTP answer's refusals: 409 and 400/404 stop, other
    /// failures retry.
    async fn refusal(&self, resp: reqwest::Response) -> Leg {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let detail = format!("HTTP {}: {}", code.as_u16(), body.trim());
        match code.as_u16() {
            409 => self.fault("cursorAhead", None, detail),
            400 | 404 | 405 | 410 => self.fault("refused", None, detail),
            _ => Leg::Transient(detail),
        }
    }

    /// Deliver one envelope at `cursor`. A fault stops the loop at the
    /// stored cursor; a storage failure is retried from it.
    async fn deliver(&mut self, cursor: &str, bytes: &[u8]) -> std::result::Result<(), Leg> {
        let Ok(at) = cursor.trim().parse::<u64>() else {
            return Err(self.fault("decode", Some(cursor), "a cursor that is not a decimal"));
        };
        let event = match decode_envelope(bytes) {
            Ok(event) => event,
            Err(fault) => return Err(self.fault("decode", Some(cursor), fault.to_string())),
        };
        let delivery = tracker_host::deliver_chain_event(
            self.storage,
            self.services,
            self.clock,
            self.opts,
            at,
            event,
        )
        .await
        .map_err(|e| Leg::Transient(format!("the delivery at {cursor} was not stored: {e}")))?;
        if at > self.status.cursor.parse().unwrap_or(0) {
            self.status.cursor = at.to_string();
            self.status.delivered += 1;
        }
        // The status is rewritten after every delivery (N = 1), so its
        // cursor is the stored cursor once this write lands, while
        // replaying and while following.
        self.status.updated_at = self.clock.now();
        write_status(self.storage, &self.status).await;
        for moved in &delivery.moved {
            tracing::info!(
                cursor = at,
                kind = delivery.kind,
                txid = %moved.txid,
                word = %moved.word,
                reask = ?moved.reask,
                suspect = moved.suspect,
                "chain feed: a chain event moved a tracked transaction"
            );
        }
        Ok(())
    }

    /// The pages from the stored cursor until `hasMore` is false.
    async fn replay(&mut self) -> std::result::Result<(), Leg> {
        loop {
            let since = self.status.cursor.clone();
            let url = format!("{}/events?since={since}", self.base);
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(|e| Leg::Transient(format!("GET /events: {e}")))?;
            if !resp.status().is_success() {
                return Err(self.refusal(resp).await);
            }
            let page: Page = match resp.json().await {
                Ok(page) => page,
                Err(e) => return Err(self.fault("decode", None, format!("a page: {e}"))),
            };
            for item in &page.events {
                let bytes = serde_json::to_vec(&item.event).unwrap_or_default();
                self.deliver(&item.cursor, &bytes).await?;
            }
            if !page.has_more {
                return Ok(());
            }
            if page.events.is_empty() || page.cursor == since {
                return Err(Leg::Transient(format!(
                    "a page at {since} said hasMore and moved no cursor"
                )));
            }
        }
    }

    /// One stream connection from the stored cursor.
    async fn stream(&mut self, reconnect: bool) -> Leg {
        let since = self.status.cursor.clone();
        let url = format!("{}/events/stream?since={since}", self.base);
        let mut request = self.client.get(&url).header("Accept", "text/event-stream");
        if reconnect {
            request = request.header("Last-Event-ID", &since);
        }
        let mut resp = match request.send().await {
            Ok(resp) => resp,
            Err(e) => return Leg::Transient(format!("GET /events/stream: {e}")),
        };
        if !resp.status().is_success() {
            return self.refusal(resp).await;
        }
        let mut buffer: Vec<u8> = Vec::new();
        let mut delivered = false;
        loop {
            let chunk = match resp.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) | Err(_) => return Leg::Dropped { delivered },
            };
            buffer.extend(chunk.iter().filter(|b| **b != b'\r'));
            while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
                let block: Vec<u8> = buffer.drain(..end + 2).collect();
                let message = SseMessage::parse(&String::from_utf8_lossy(&block));
                if message.event.as_deref() == Some("error") {
                    return self.fault(
                        "feedUnavailable",
                        message.id.as_deref(),
                        message.data.unwrap_or_default(),
                    );
                }
                let Some(data) = message.data else {
                    continue;
                };
                let Some(id) = message.id else {
                    return self.fault("decode", None, format!("an envelope with no id: {data}"));
                };
                if let Err(leg) = self.deliver(&id, data.as_bytes()).await {
                    return leg;
                }
                delivered = true;
            }
        }
    }
}

/// `GET /events` (the page carrier, version 1).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    cursor: String,
    events: Vec<PageItem>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct PageItem {
    cursor: String,
    event: serde_json::Value,
}

/// One SSE message: its `id`, `event` and joined `data` lines; comments
/// (heartbeats) are dropped.
#[derive(Debug, Default, PartialEq, Eq)]
struct SseMessage {
    id: Option<String>,
    event: Option<String>,
    data: Option<String>,
}

impl SseMessage {
    fn parse(block: &str) -> Self {
        let mut message = Self::default();
        for line in block.lines() {
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "id" => message.id = Some(value.to_string()),
                "event" => message.event = Some(value.to_string()),
                "data" => {
                    message.data = Some(match message.data.take() {
                        Some(previous) => format!("{previous}\n{value}"),
                        None => value.to_string(),
                    })
                }
                _ => {}
            }
        }
        message
    }
}

#[cfg(test)]
#[path = "chain_feed_tests.rs"]
mod tests;
