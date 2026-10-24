//! `list_outputs` (#71): `listOutputs` of the default basket, limit 100, on
//! a wallet of 1,000 and of 10,000 coins.

mod common;

use std::time::Duration;

use bsv_sdk::wallet::{ListOutputsArgs, WalletInterface};
use common::{configure, measure_calls, runtime, timed, wallet_of, SIZES};
use criterion::{criterion_group, criterion_main, Criterion};

fn args() -> ListOutputsArgs {
    ListOutputsArgs {
        basket: "default".to_string(),
        limit: Some(100),
        offset: None,
        include: None,
        include_custom_instructions: None,
        include_labels: None,
        include_tags: None,
        tags: None,
        tag_query_mode: None,
        seek_permission: None,
    }
}

fn list_outputs(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("list_outputs");
    for coins in SIZES {
        let fixture = rt.block_on(wallet_of(coins));
        let wallet = &fixture.wallet;
        // ~0.7 ms and ~3 ms a call on the lane's machine: thousands of
        // calls in 10 s.
        configure(&mut group, Duration::from_secs(2), Duration::from_secs(10));
        measure_calls(
            &mut group,
            &rt,
            ("list_outputs", "limit100", coins),
            async |calls| {
                let (result, d) = timed(calls, wallet.list_outputs(args(), "bench.local")).await;
                assert_eq!(result.unwrap().outputs.len(), 100);
                d
            },
            async || {},
        );
    }
    group.finish();
}

criterion_group!(benches, list_outputs);
criterion_main!(benches);
