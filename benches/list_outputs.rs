//! `list_outputs` (#71): `listOutputs` of the default basket, limit 100, on
//! a wallet of 1,000 and of 10,000 coins.

mod common;

use std::time::Duration;

use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use common::{configure, report, runtime, timed, wallet_of, Calls, SIZES};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn args() -> ListOutputsArgs {
    ListOutputsArgs {
        basket: "default".to_string(),
        tags: None,
        tag_query_mode: None,
        include: None,
        include_custom_instructions: None,
        include_tags: None,
        include_labels: None,
        limit: Some(100),
        offset: None,
        seek_permission: None,
    }
}

fn list_outputs(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("list_outputs");
    for coins in SIZES {
        let fixture = rt.block_on(wallet_of(coins));
        let wallet = &fixture.wallet;
        let samples = 50;
        configure(
            &mut group,
            samples,
            Duration::from_secs(2),
            Duration::from_secs(10),
        );
        let calls = Calls::new();
        group.bench_function(BenchmarkId::new("limit100", coins), |b| {
            b.iter_custom(|iters| {
                calls.routine();
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let (result, d) =
                            timed(&calls, wallet.list_outputs(args(), "bench.local")).await;
                        assert_eq!(result.unwrap().outputs.len(), 100);
                        total += d;
                    }
                    total
                })
            })
        });
        report(&format!("list_outputs/limit100/{coins}"), &calls, samples);
    }
    group.finish();
}

criterion_group!(benches, list_outputs);
criterion_main!(benches);
