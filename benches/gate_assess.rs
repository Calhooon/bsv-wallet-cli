//! `gate_assess` (#71): the spend gate's assessment of a wallet of 1,000
//! and of 10,000 coins, the mock header service answering. `need` is a
//! payment's amount (the door's lazy read: the gate stops once the coins
//! offered cover it); `all` is `None` (every coin read and judged).

mod common;

use std::time::Duration;

use bsv_wallet_cli::spend_gate::{assess, GateOptions};
use common::{configure, measure_calls, runtime, timed, wallet_of, SIZES};
use criterion::{criterion_group, criterion_main, Criterion};

/// A payment of 10,000 sats.
const NEED: u64 = 10_000;

fn gate_assess(c: &mut Criterion) {
    let rt = runtime();
    let opts = GateOptions::default();
    let mut group = c.benchmark_group("gate_assess");
    for coins in SIZES {
        let fixture = rt.block_on(wallet_of(coins));
        let wallet = &fixture.wallet;
        for (name, need) in [("need", Some(NEED)), ("all", None)] {
            // Calls for a stable p99, never fewer than the 100 samples: about
            // 3,000 at 1,000 coins (p50 ~3 ms and ~20 ms on the lane's
            // machine), about 300 at 10,000 (~80 ms and ~260 ms; 100 calls
            // of `all` need 26 s), so a CI run of every bench stays near
            // ten minutes.
            let measure = match (need, coins) {
                (Some(_), 1_000) => Duration::from_secs(10),
                (None, 1_000) => Duration::from_secs(25),
                (Some(_), _) => Duration::from_secs(30),
                (None, _) => Duration::from_secs(90),
            };
            configure(&mut group, Duration::from_secs(2), measure);
            measure_calls(
                &mut group,
                &rt,
                ("gate_assess", name, coins),
                async |calls| {
                    let (a, d) = timed(
                        calls,
                        assess(wallet.storage(), wallet.services(), &opts, need),
                    )
                    .await;
                    assert!(a.unwrap().offered_satoshis() >= NEED);
                    wallet.services().reset_history();
                    d
                },
                async || {},
            );
        }
    }
    group.finish();
}

criterion_group!(benches, gate_assess);
criterion_main!(benches);
