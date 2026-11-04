//! The owner's step (#414, the design's L5b fixtures): what the owner does
//! around a fixture's request, through the store module as the CLI does,
//! so a held call's answer is recorded and replayed as the server serves
//! it. A fixture names it as `"owner"`:
//!
//! - `before`: grants and revokes written before the request (`{"grant":
//!   {"originator", "kind", "key"?, "expiry"?}}`, `{"revoke":
//!   {"originator", "kind"?, "key"?}}`; an `expiry` of 1 is long past),
//!   and a row its caller left (`{"abandon": {"originator", "kind", "key",
//!   "usage", "call"}}`, #534: the power's pending row opened as a held call
//!   opens it, then marked as the disconnect marks it, its window open);
//! - `holding`: calls sent first and held, their answers not recorded (the
//!   cap's fixture); each is denied once the request is answered;
//! - `spent`: a call sent `times` times first, each answered 200 and not
//!   recorded (the open doors' bound's fixtures, #541: the bound's
//!   allowance spent, so the request is the call past it);
//! - `after_ms` and `answer`: once the request's row is open, after that
//!   many milliseconds, every open row answered `grant`, `once` or `deny`,
//!   or the server shut down (`shutdown`); `amount`, a spending grant's
//!   ceiling (#414 L6).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use bsv_wallet_cli::server::handlers::WalletState;
use bsv_wallet_cli::server::powers::Held;
use bsv_wallet_cli::store::origin_grants::{self as grants, Answer, Asking, Granting, Kind, Power};
use bsv_wallet_cli::store::origin_grants::{Reach, Reply};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use super::fixture::NORMALIZER;
use super::worlds::World;
use crate::wire_support::{Req, Resp, J};

/// The owner's step of one fixture.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owner {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) before: Vec<Act>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) holding: Vec<Req>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spent: Option<Spent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) amount: Option<i64>,
}

/// A call sent `times` times before the request, each answered 200.
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spent {
    pub(crate) times: u32,
    pub(crate) call: Req,
}

/// A row the owner writes before the request.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum Act {
    Grant {
        originator: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default)]
        expiry: i64,
    },
    Revoke {
        originator: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    Abandon {
        originator: String,
        kind: String,
        key: String,
        usage: String,
        call: String,
    },
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// The store's open rows now.
async fn open(wallet: &WalletState) -> Vec<i64> {
    grants::listing(wallet.storage(), false, now())
        .await
        .unwrap()
        .pending
        .iter()
        .map(|r| r.pending_id)
        .collect()
}

/// Wait until `n` calls are held (the cap's count, in all).
async fn until_held(held: &Held, n: usize) {
    for _ in 0..500 {
        if held.counts("").1 == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the owner's step: {n} held calls never appeared");
}

/// Answer every open row `answer` (a spending grant's ceiling `amount`),
/// waking the calls held on them.
async fn answer_open(wallet: &WalletState, held: &Held, answer: Answer, amount: Option<i64>) {
    let reply = Reply {
        answer,
        expiry: 0,
        amount,
    };
    for id in open(wallet).await {
        grants::answer(wallet.storage(), id, &reply, now())
            .await
            .unwrap();
        held.wake(id);
    }
}

/// A row its caller left (#534): the power's pending row opened as a held
/// call opens it, then marked as the disconnect marks it.
async fn abandon(
    wallet: &WalletState,
    originator: &str,
    power: (&str, &str),
    usage: &str,
    call: &str,
) {
    let power = Power::named(Kind::named(power.0).unwrap(), Some(power.1)).unwrap();
    let asking = Asking {
        originator,
        power: &power,
        usage: Some(usage),
        call,
        reason: None,
        amount: None,
        renewal: false,
    };
    let at = now();
    let opened = grants::open_or_join(wallet.storage(), &asking, at)
        .await
        .unwrap();
    let id = opened.row.pending_id;
    let done = grants::abandon(wallet.storage(), originator, &[id], at)
        .await
        .unwrap();
    assert_eq!(
        done.marked.len(),
        1,
        "the owner's step: row #{id} not marked"
    );
}

impl Owner {
    /// The rows written before the request.
    pub(crate) async fn before(&self, wallet: &WalletState) {
        for act in &self.before {
            match act {
                Act::Grant {
                    originator,
                    kind,
                    key,
                    expiry,
                } => {
                    let power = Power::named(Kind::named(kind).unwrap(), key.as_deref()).unwrap();
                    let granting = Granting {
                        originator,
                        power: &power,
                        amount: None,
                        expiry: *expiry,
                    };
                    grants::grant(wallet.storage(), &granting, now())
                        .await
                        .unwrap();
                }
                Act::Revoke {
                    originator,
                    kind,
                    key,
                } => {
                    let kind = kind.as_deref().map(|k| Kind::named(k).unwrap());
                    let reach = Reach::named(kind, key.as_deref()).unwrap();
                    grants::revoke(wallet.storage(), originator, &reach, now())
                        .await
                        .unwrap();
                }
                Act::Abandon {
                    originator,
                    kind,
                    key,
                    usage,
                    call,
                } => abandon(wallet, originator, (kind, key), usage, call).await,
            }
        }
    }

    /// The spent calls sent first, one after another, each answered 200.
    pub(crate) async fn spend(&self, router: &Router) {
        let Some(spent) = &self.spent else {
            return;
        };
        for n in 0..spent.times {
            let answer = router.clone().oneshot(spent.call.to_http()).await.unwrap();
            assert_eq!(
                answer.status().as_u16(),
                200,
                "the owner's step: spent call {n} of {} {} was not answered 200",
                spent.times,
                spent.call.path
            );
        }
    }

    /// The held calls sent first: each held before the request is sent.
    pub(crate) async fn hold(
        &self,
        router: &Router,
        held: &Held,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut calls = Vec::new();
        for (n, req) in self.holding.iter().enumerate() {
            let (router, http) = (router.clone(), req.to_http());
            calls.push(tokio::spawn(async move {
                let _ = router.oneshot(http).await;
            }));
            until_held(held, n + 1).await;
        }
        calls
    }

    /// The owner's act once the request's row is open: after `after_ms`,
    /// every open row answered, or the server shut down.
    pub(crate) async fn act(&self, wallet: &WalletState, held: &Held) {
        let Some(answer) = self.answer.as_deref() else {
            return;
        };
        let waiting = self.holding.len() + 1;
        until_held(held, waiting).await;
        tokio::time::sleep(Duration::from_millis(self.after_ms.unwrap_or(0))).await;
        match answer {
            "shutdown" => held.shut_down(),
            "grant" => answer_open(wallet, held, Answer::Grant, self.amount).await,
            "once" => answer_open(wallet, held, Answer::Once, None).await,
            "deny" => answer_open(wallet, held, Answer::Deny, None).await,
            other => panic!("the owner's step: {other:?} is not grant, once, deny or shutdown"),
        }
    }

    /// After the request is answered: every call still held denied, and
    /// awaited.
    pub(crate) async fn after(
        &self,
        wallet: &WalletState,
        held: &Held,
        calls: Vec<tokio::task::JoinHandle<()>>,
    ) {
        if calls.is_empty() {
            return;
        }
        answer_open(wallet, held, Answer::Deny, None).await;
        for call in calls {
            call.await.unwrap();
        }
    }
}

impl World {
    /// The normalized response, and the JSON body as served, once the
    /// probe record holds the answers served before it (#539: its writer
    /// is off the call's path).
    pub(crate) async fn serve_raw(&self, req: &Req) -> (Resp, Option<J>) {
        self.probes.settled().await;
        NORMALIZER.serve(&self.router, req).await
    }

    /// The world's `serve_raw` with the owner's step around the request:
    /// the rows written before it, the calls held first, the owner's act
    /// while it is held, then every call still held denied.
    pub(crate) async fn serve_owned(&self, req: &Req, owner: Option<&Owner>) -> (Resp, Option<J>) {
        let Some(owner) = owner else {
            return self.serve_raw(req).await;
        };
        self.probes.settled().await;
        owner.before(&self.wallet).await;
        owner.spend(&self.router).await;
        let calls = owner.hold(&self.router, &self.held).await;
        let (served, ()) = tokio::join!(
            NORMALIZER.serve(&self.router, req),
            owner.act(&self.wallet, &self.held)
        );
        owner.after(&self.wallet, &self.held, calls).await;
        served
    }
}
