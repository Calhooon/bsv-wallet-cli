//! `create_action_signed` (#71): the `/createAction` handler's path from
//! the spending lock to a signed transaction, without the JSON and without
//! the network: the lock (held by no one else), the spend guard, then the
//! spend gate's door to the toolbox with `noSend` (signed, not broadcast).
//! Each action is aborted after it is timed, so its coin comes back and the
//! wallet keeps its size.

mod common;

use std::sync::Arc;
use std::time::Duration;

use bsv_sdk::primitives::to_hex;
use bsv_sdk::wallet::{
    AbortActionArgs, CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface,
};
use bsv_wallet_cli::server::SpendingLock;
use bsv_wallet_cli::spend_gate::{create_action, GateOptions};
use common::{configure, report, runtime, timed, wallet_of, Calls, LOCK, SIZES};
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
        let wallet = &fixture.wallet;
        // Calls for a stable p99: about 1,000 at either size.
        let (samples, measure) = match coins {
            1_000 => (50, Duration::from_secs(15)),
            _ => (30, Duration::from_secs(90)),
        };
        configure(&mut group, samples, Duration::from_secs(2), measure);
        let calls = Calls::new();
        group.bench_function(BenchmarkId::new("signed", coins), |b| {
            b.iter_custom(|iters| {
                calls.routine();
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let (result, d) = timed(&calls, async {
                            let _guard = lock.lock().await;
                            bsv_wallet_cli::spend_guard::run(wallet.storage())
                                .await
                                .unwrap();
                            create_action(wallet, pay(), ORIGINATOR, &opts).await
                        })
                        .await;
                        let result = result.unwrap();
                        total += d;
                        let txid = to_hex(&result.txid.expect("a signed txid"));
                        wallet
                            .abort_action(AbortActionArgs { reference: txid }, ORIGINATOR)
                            .await
                            .unwrap();
                        wallet.services().reset_history();
                    }
                    total
                })
            })
        });
        report(
            &format!("create_action_signed/signed/{coins}"),
            &calls,
            samples,
        );
    }
    group.finish();
}

criterion_group!(benches, create_action_signed);
criterion_main!(benches);
