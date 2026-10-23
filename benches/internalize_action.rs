//! `internalize_action` (#71): the `/internalizeAction` handler's path
//! without the JSON: one BRC-29 payment (the `wallet payment` protocol, a
//! fresh derivation each call) whose AtomicBEEF carries its proof, the
//! toolbox's `internalizeAction`, then the route's post word. The payment
//! is built before the clock starts; the wallet grows by one coin a call.

mod common;

use std::time::Duration;

use bsv_sdk::primitives::{to_base64, PrivateKey};
use bsv_sdk::script::LockingScript;
use bsv_sdk::transaction::{Beef, MerklePath, Transaction, TransactionInput, TransactionOutput};
use bsv_sdk::wallet::{
    Counterparty, InternalizeActionArgs, InternalizeOutput, KeyDeriver, Protocol, SecurityLevel,
    WalletInterface, WalletPayment,
};
use bsv_wallet_cli::server::post_word::{atomic_subject, post_word};
use common::{configure, report, runtime, timed, wallet_of, Calls, SIZES};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

/// BRC-29's protocol id.
const BRC29: &str = "3241645161d8";
const PREFIX: &str = "SfKxPIJNgdI=";

/// Payment `n` from `sender` to the wallet of `root`: its arguments.
fn payment(root: &PrivateKey, sender: &PrivateKey, n: u64) -> InternalizeActionArgs {
    let suffix = to_base64(&n.to_le_bytes());
    let deriver = KeyDeriver::new(Some(root.clone()));
    let key = deriver
        .derive_public_key(
            &Protocol::new(SecurityLevel::Counterparty, BRC29),
            &format!("{PREFIX} {suffix}"),
            &Counterparty::Other(sender.public_key()),
            true,
        )
        .unwrap();
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&key.hash160());
    script.extend_from_slice(&[0x88, 0xac]);

    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&n.to_le_bytes());
    seed[31] = 0x29;
    let mut tx = Transaction::new();
    tx.add_input(TransactionInput::new(hex::encode(seed), 0))
        .unwrap();
    tx.add_output(TransactionOutput::new(
        5_000,
        LockingScript::from_binary(&script).unwrap(),
    ))
    .unwrap();
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
        let wallet = &fixture.wallet;
        let samples = 50;
        configure(
            &mut group,
            samples,
            Duration::from_secs(2),
            Duration::from_secs(15),
        );
        let calls = Calls::new();
        group.bench_function(BenchmarkId::new("brc29", coins), |b| {
            b.iter_custom(|iters| {
                calls.routine();
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        next += 1;
                        let args = payment(&fixture.root, &sender, next);
                        let (accepted, d) = timed(&calls, async {
                            let subject = atomic_subject(&args.tx);
                            let result = wallet.internalize_action(args, "bench.local").await;
                            if let Some(txid) = subject {
                                post_word(wallet.storage(), &txid).await;
                            }
                            result
                        })
                        .await;
                        assert!(accepted.unwrap().accepted);
                        total += d;
                        wallet.services().reset_history();
                    }
                    total
                })
            })
        });
        report(
            &format!("internalize_action/brc29/{coins}"),
            &calls,
            samples,
        );
    }
    group.finish();
}

criterion_group!(benches, internalize_action);
criterion_main!(benches);
