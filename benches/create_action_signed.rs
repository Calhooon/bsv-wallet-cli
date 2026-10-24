//! `create_action_signed` (#71): the `/createAction` handler's path from
//! the spending lock to a signed transaction, without the JSON and without
//! the network: the lock (held by no one else), the spend guard, then the
//! spend gate's door to the toolbox with `noSend` (signed, not broadcast).
//! Each call is on its own copy of the built wallet
//! ([`common::Fixture::fresh`], made before the clock starts), so the wallet
//! holds exactly 1,000 (or 10,000) coins and no earlier action when every
//! call starts.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bsv_sdk::wallet::{CreateActionArgs, CreateActionOptions, CreateActionOutput};
use bsv_wallet_cli::server::SpendingLock;
use bsv_wallet_cli::spend_gate::{create_action, GateOptions};
use common::{coin_count, configure, report, runtime, timed, wallet_of, Calls, LOCK, SIZES};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

const ORIGINATOR: &str = "bench.local";

/// A payment of 1,000 sats, signed and not broadcast.
fn pay() -> CreateActionArgs {
    CreateActionArgs {
        description: "bench payment".to_string(),
        input_beef: None,
        inputs: Some(vec![]),
        outputs: Some(vec![CreateActionOutput {
            locking_script: hex::decode(LOCK).unwrap(),
            satoshis: 1_000,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: None,
        }]),
        lock_time: None,
        version: None,
        labels: None,
        options: Some(CreateActionOptions {
            sign_and_process: Some(true),
            no_send: Some(true),
            ..Default::default()
        }),
    }
}

fn create_action_signed(c: &mut Criterion) {
    let rt = runtime();
    let opts = GateOptions::default();
    let lock: SpendingLock = Arc::new(tokio::sync::Mutex::new(()));
    let mut group = c.benchmark_group("create_action_signed");
    for coins in SIZES {
        let fixture = rt.block_on(wallet_of(coins));
        // Calls for a stable p99, never fewer than the 100 samples: ~8 ms a
        // call at 1,000 coins on the lane's machine (8 s is about 1,000
        // calls), ~130 ms at 10,000 (100 calls need 13 s; 30 s is about
        // 230). Each call's copy of the database is untimed wall clock.
        let measure = match coins {
            1_000 => Duration::from_secs(8),
            _ => Duration::from_secs(30),
        };
        configure(&mut group, Duration::from_secs(2), measure);
        let calls = Calls::new();
        group.bench_function(BenchmarkId::new("signed", coins), |b| {
            b.iter_custom(|iters| {
                calls.routine();
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let fresh = fixture.fresh().await;
                        let wallet = &fresh.wallet;
                        assert_eq!(coin_count(wallet).await, coins);
                        let (result, d) = timed(&calls, async {
                            let _guard = lock.lock().await;
                            bsv_wallet_cli::spend_guard::run(wallet.storage())
                                .await
                                .unwrap();
                            create_action(wallet, pay(), ORIGINATOR, &opts).await
                        })
                        .await;
                        assert!(result.unwrap().txid.is_some(), "a signed txid");
                        total += d;
                    }
                    assert_eq!(coin_count(&fixture.fresh().await.wallet).await, coins);
                    total
                })
            })
        });
        report(&format!("create_action_signed/signed/{coins}"), &calls);
    }
    group.finish();
}

criterion_group!(benches, create_action_signed);
criterion_main!(benches);
