//! `internalize_action` (#71): the `/internalizeAction` handler's path
//! without the JSON: one BRC-29 payment (the `wallet payment` protocol, a
//! fresh derivation each call) whose `AtomicBEEF` carries its proof, the
//! toolbox's `internalizeAction`, then the route's post word. Each call is
//! on its own copy of the built wallet ([`common::Fixture::fresh`]), so the
//! wallet holds exactly 1,000 (or 10,000) coins when every call starts; the
//! copy and the payment are made before the clock starts.

mod common;

use std::time::Duration;

use bsv_sdk::primitives::{to_base64, PrivateKey};
use bsv_sdk::transaction::{Beef, MerklePath};
use bsv_sdk::wallet::{
    Counterparty, InternalizeActionArgs, InternalizeOutput, KeyDeriver, WalletInterface,
    WalletPayment,
};
use bsv_wallet_cli::server::post_word::{atomic_subject, post_word};
use common::{
    brc29_lock, coin_count, configure, measure_calls, parent, runtime, timed, wallet_of, PREFIX,
    SIZES,
};
use criterion::{criterion_group, criterion_main, Criterion};

/// Payment `n` from `sender` to the wallet of `root`: its arguments.
fn payment(root: &PrivateKey, sender: &PrivateKey, n: u64) -> InternalizeActionArgs {
    let suffix = to_base64(&n.to_le_bytes());
    let deriver = KeyDeriver::new(Some(root.clone()));
    let script = brc29_lock(&deriver, &suffix, &Counterparty::Other(sender.public_key()));
    let tx = parent(n, 0x29, 5_000, &script);
    let txid = tx.id();
    let mut beef = Beef::new();
    let bump = beef.merge_bump(MerklePath::from_coinbase_txid(&txid, 850_000));
    beef.merge_raw_tx(tx.to_binary(), Some(bump));
    InternalizeActionArgs {
        tx: beef.to_binary_atomic(&txid).unwrap(),
        outputs: vec![InternalizeOutput {
            output_index: 0,
            protocol: "wallet payment".to_string(),
            payment_remittance: Some(WalletPayment {
                derivation_prefix: PREFIX.to_string(),
                derivation_suffix: suffix,
                sender_identity_key: sender.public_key().to_hex(),
            }),
            insertion_remittance: None,
        }],
        description: "bench payment in".to_string(),
        labels: None,
        seek_permission: None,
    }
}

fn internalize_action(c: &mut Criterion) {
    let rt = runtime();
    let sender = PrivateKey::random();
    let mut next: u64 = 0;
    let mut group = c.benchmark_group("internalize_action");
    for coins in SIZES {
        let fixture = rt.block_on(wallet_of(coins));
        // ~1.5 ms and ~2.5 ms a call on the lane's machine: 3 s of timed
        // calls is about 1,000 to 2,000 calls, each after its own copy of
        // the database (untimed, and the bench's wall clock).
        configure(&mut group, Duration::from_secs(2), Duration::from_secs(3));
        measure_calls(
            &mut group,
            &rt,
            ("internalize_action", "brc29", coins),
            async |calls| {
                next += 1;
                let args = payment(&fixture.root, &sender, next);
                let fresh = fixture.fresh().await;
                let wallet = &fresh.wallet;
                assert_eq!(coin_count(wallet).await, coins);
                let (accepted, d) = timed(calls, async {
                    let subject = atomic_subject(&args.tx);
                    let result = wallet.internalize_action(args, "bench.local").await;
                    if let Some(txid) = subject {
                        post_word(wallet.storage(), &txid).await;
                    }
                    result
                })
                .await;
                assert!(accepted.unwrap().accepted);
                // The payment landed in this call's copy only.
                assert_eq!(coin_count(wallet).await, coins + 1);
                d
            },
            async || fixture.assert_as_built().await,
        );
    }
    group.finish();
}

criterion_group!(benches, internalize_action);
criterion_main!(benches);
