//! The toolbox's two `internalizeAction` rules on this line, served (the
//! port of the stable line's 0.18.4 and 0.18.5): the wallet payment rule
//! (toolbox #819) and the basket insertion rules (toolbox #823), each
//! answer read through `POST /internalizeAction` in the `internalize`
//! world, kept apart so its listings show only what these rows stored.
//! Recorded last, so no file before them moves.
use serde_json::{json, Value};

use super::cases::exact;
use super::recorder::{post, Recorded, Recorder};
use super::worlds::{
    funded_lock, legacy_default, parent_tx, stranger_key, stranger_payment, Payment, FUNDED_COINS,
    P2PKH, PAYMENT_PREFIX, PAYMENT_SUFFIX,
};
use crate::wire_support::{BOUND, ROOT_KEY};

const WORLD: &str = "internalize";

pub(crate) async fn record(r: &mut Recorder) {
    r.case(
        "listOutputs.internalize.seeded",
        WORLD,
        post("/listOutputs", json!({"basket": "default"})),
    )
    .await;
    record_payment_rule(r).await;
    record_default_basket(r).await;
    let nothing = r
        .case(
            "listActions.internalize.nothingStored",
            WORLD,
            post("/listActions", json!({"labels": []})),
        )
        .await;
    // The world's six seeded transactions alone: no refusal above wrote one.
    assert_eq!(nothing.value("/totalActions"), 6);
    record_baskets(r).await;
    record_managed_change(r).await;
    record_sweeps(r).await;
}

/// A wallet payment output of `payment`'s transaction, the stranger's
/// remittance.
fn wallet_payment() -> Value {
    json!({"outputIndex": 0, "protocol": "wallet payment",
        "paymentRemittance": {"derivationPrefix": PAYMENT_PREFIX, "derivationSuffix": PAYMENT_SUFFIX,
            "senderIdentityKey": stranger_key().public_key().to_hex()}})
}

/// A basket insertion of output 0 into `basket`.
fn insertion(basket: &str) -> Value {
    json!({"outputIndex": 0, "protocol": "basket insertion",
        "insertionRemittance": {"basket": basket}})
}

/// `POST /internalizeAction` of `tx` with one `output`.
async fn internalize(r: &mut Recorder, name: &str, tx: &[u8], output: Value) -> Recorded {
    r.case(
        name,
        WORLD,
        post(
            "/internalizeAction",
            json!({"tx": tx, "outputs": [output], "description": "wire internalize rules"}),
        ),
    )
    .await
}

/// A [`Payment`]'s `AtomicBEEF`.
async fn beef(payment: Payment) -> Vec<u8> {
    let key = bsv_sdk::primitives::PrivateKey::from_hex(ROOT_KEY).unwrap();
    stranger_payment(payment, &key).await.1
}

/// Toolbox #819: an output that carries a payment remittance is admitted
/// only when it is locked by the P2PKH of the key the remittance derives,
/// read from the BEEF's subject unlinked.
async fn record_payment_rule(r: &mut Recorder) {
    // An AtomicBEEF whose header names a subject the BEEF does not carry.
    let mut atomic = beef(Payment::OtherKey).await;
    atomic[4..36].copy_from_slice(&[0x5a; 32]);
    internalize(
        r,
        "internalizeAction.payment.subjectNotCarried",
        &atomic,
        wallet_payment(),
    )
    .await;
    for (name, payment) in [
        ("otherKey", Payment::OtherKey),
        ("derivedP2pk", Payment::DerivedP2pk),
        ("opReturn", Payment::OpReturn),
        ("derivedP2pkhAndByte", Payment::DerivedP2pkhAndByte),
        ("emptyScript", Payment::EmptyScript),
    ] {
        let atomic = beef(payment).await;
        internalize(
            r,
            &format!("internalizeAction.payment.{name}"),
            &atomic,
            wallet_payment(),
        )
        .await;
    }
}

/// Toolbox #823: the default basket is never a basket insertion's basket,
/// in the spellings the toolbox's line names.
async fn record_default_basket(r: &mut Recorder) {
    let atomic = beef(Payment::Insertions).await;
    for (name, basket) in [
        ("exact", "default"),
        ("upper", "DEFAULT"),
        ("spaced", " Default "),
    ] {
        internalize(
            r,
            &format!("internalizeAction.insertion.default.{name}"),
            &atomic,
            insertion(basket),
        )
        .await;
    }
}

/// Toolbox #823: a named basket; the same basket named again; another
/// application basket refused, and not created.
async fn record_baskets(r: &mut Recorder) {
    let atomic = beef(Payment::Insertions).await;
    for (name, basket) in [
        ("named", "tokens"),
        ("sameBasket", "tokens"),
        ("anotherBasket", "other"),
    ] {
        internalize(
            r,
            &format!("internalizeAction.insertion.{name}"),
            &atomic,
            insertion(basket),
        )
        .await;
    }
    for basket in ["tokens", "other"] {
        r.case(
            &format!("listOutputs.internalize.{basket}"),
            WORLD,
            post("/listOutputs", json!({"basket": basket})),
        )
        .await;
    }
}

/// Toolbox #819's accepting side, then #823 over wallet-managed change: a
/// received payment, and a coin of the wallet's own change; and a wallet
/// payment that carries a stray insertion remittance naming `default`.
async fn record_managed_change(r: &mut Recorder) {
    let received = beef(Payment::Wallet).await;
    internalize(
        r,
        "internalizeAction.payment.derived",
        &received,
        wallet_payment(),
    )
    .await;
    internalize(
        r,
        "internalizeAction.insertion.overReceivedPayment",
        &received,
        insertion("tokens"),
    )
    .await;
    internalize(
        r,
        "internalizeAction.insertion.overOwnChange",
        &own_change(1),
        insertion("tokens"),
    )
    .await;
    let mut stray = wallet_payment();
    stray["insertionRemittance"] = json!({"basket": "default"});
    internalize(
        r,
        "internalizeAction.payment.strayInsertionDefault",
        &beef(Payment::StrayInsertion).await,
        stray,
    )
    .await;
}

/// The `AtomicBEEF` of the funded coin `i`'s transaction (its one-leaf
/// proof): a change row of the wallet's own (`P2PKH`, change, `provided_by`
/// `storage`, `purpose` `change`, its derivation).
fn own_change(i: usize) -> Vec<u8> {
    let key = bsv_sdk::primitives::PrivateKey::from_hex(ROOT_KEY).unwrap();
    let (seed, height, satoshis, suffix) = FUNDED_COINS[i];
    let parent = parent_tx(seed, satoshis, &funded_lock(&key, suffix));
    let mut beef = bsv_sdk::transaction::Beef::new();
    beef.merge_bump(bsv_sdk::transaction::MerklePath::from_coinbase_txid(
        &parent.id(),
        height,
    ));
    beef.merge_transaction(parent.clone());
    beef.to_binary_atomic(&parent.id()).unwrap()
}

/// Toolbox #823: an output in no basket (an output of the wallet's own
/// action), and one in the default basket and not change (as an earlier
/// release's insertion into `default` left it), swept into a named basket;
/// another such row relinquished from `default`; what `default` and the
/// named basket then list.
async fn record_sweeps(r: &mut Recorder) {
    let action = r
        .case(
            "createAction.internalize.noBasket",
            WORLD,
            post(
                "/createAction",
                json!({"description": "wire output in no basket", "outputs": [
                    {"lockingScript": P2PKH, "satoshis": exact(0), "outputDescription": "an output"}],
                    "options": {"acceptDelayedBroadcast": false, "randomizeOutputs": false}}),
            ),
        )
        .await;
    r.case(
        "internalizeAction.insertion.sweepNoBasket",
        WORLD,
        post(
            "/internalizeAction",
            json!({"tx": BOUND, "outputs": [insertion("swept")],
                "description": "wire internalize rules"}),
        )
        .bind("/tx", &action, "/tx"),
    )
    .await;
    internalize(
        r,
        "internalizeAction.insertion.sweepDefaultNotChange",
        &legacy_default(0).1,
        insertion("swept"),
    )
    .await;
    r.case(
        "relinquishOutput.internalize.legacyDefault",
        WORLD,
        post(
            "/relinquishOutput",
            json!({"basket": "default", "output": format!("{}.0", legacy_default(1).0.id())}),
        ),
    )
    .await;
    for basket in ["default", "swept"] {
        r.case(
            &format!("listOutputs.internalize.{basket}"),
            WORLD,
            post("/listOutputs", json!({"basket": basket})),
        )
        .await;
    }
}

/// The rows' notes in the manifest, each naming the toolbox's rule.
pub(crate) fn notes() -> Vec<(String, String)> {
    [
        ("222-listOutputs.internalize.seeded.json", "toolbox #819, #823: the internalize world, kept apart so its listings show only what these rows stored: funded's four coins and two rows as an earlier release's basket insertion into default left them (custom, not change), each listed in default beside the coins. New; recorded last, so no file before it moves."),
        ("223-internalizeAction.payment.subjectNotCarried.json", "toolbox #819: an AtomicBEEF naming a subject it does not carry, with a wallet payment output: the subject cannot be read, a WalletRefusal InvalidParameter served 400 VALIDATION_ERROR by its class; nothing stored (232)."),
        ("224-internalizeAction.payment.otherKey.json", "toolbox #819: a wallet payment output locked by the P2PKH of another key than its remittance derives: Derivation mismatch, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (232)."),
        ("225-internalizeAction.payment.derivedP2pk.json", "toolbox #819: a wallet payment output that is not the P2PKH of the derived key (here the derived key's P2PK): the BRC-29 sentence, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (232)."),
        ("226-internalizeAction.payment.opReturn.json", "toolbox #819: as 225, an OP_RETURN script."),
        ("227-internalizeAction.payment.derivedP2pkhAndByte.json", "toolbox #819: as 225, the derived key's P2PKH with one byte after it."),
        ("228-internalizeAction.payment.emptyScript.json", "toolbox #819: as 225, an empty script."),
        ("229-internalizeAction.insertion.default.exact.json", "toolbox #823: a basket insertion naming the default basket is refused before anything is written, InvalidParameter, 400 VALIDATION_ERROR; nothing stored (232)."),
        ("230-internalizeAction.insertion.default.upper.json", "toolbox #823: as 229, DEFAULT."),
        ("231-internalizeAction.insertion.default.spaced.json", "toolbox #823: as 229, a space around Default."),
        ("232-listActions.internalize.nothingStored.json", "toolbox #819, #823: after 223 to 231, the world's six seeded transactions alone: no refusal wrote a transaction."),
        ("233-internalizeAction.insertion.named.json", "toolbox #823: a basket insertion into a named basket is admitted, 200."),
        ("234-internalizeAction.insertion.sameBasket.json", "toolbox #823: the same basket named again is admitted, 200."),
        ("235-internalizeAction.insertion.anotherBasket.json", "toolbox #823: an output already in another application basket is never moved into another: InvalidParameter, 400 VALIDATION_ERROR; the output stays in its basket (236) and the named basket is not created (237)."),
        ("236-listOutputs.internalize.tokens.json", "toolbox #823: after 235, the output stays in tokens."),
        ("237-listOutputs.internalize.other.json", "toolbox #823: after 235, basket other holds nothing."),
        ("238-internalizeAction.payment.derived.json", "toolbox #819: a wallet payment output locked by the P2PKH of the derived key is admitted, 200, and stored as change of the default basket (246)."),
        ("239-internalizeAction.insertion.overReceivedPayment.json", "toolbox #823: a basket insertion over a received payment (238), wallet-managed change, is refused: InvalidParameter, 400 VALIDATION_ERROR; the payment stays in default (246)."),
        ("240-internalizeAction.insertion.overOwnChange.json", "toolbox #823: as 239, over a coin of the wallet's own change (P2PKH, change, provided_by storage, purpose change, its derivation); the coin stays in default (246)."),
        ("241-internalizeAction.payment.strayInsertionDefault.json", "toolbox #823: a wallet payment carrying an insertion remittance that names default is refused by the argument validator, as 154 refuses the converse: InvalidParameter, 400 VALIDATION_ERROR; nothing stored (246)."),
        ("242-createAction.internalize.noBasket.json", "toolbox #823: an action paying its coin less the fee to P2PKH, so its output is in no basket (no change output is drawn)."),
        ("243-internalizeAction.insertion.sweepNoBasket.json", "toolbox #823: an output in no basket (242's) is swept into a named basket, 200 (247)."),
        ("244-internalizeAction.insertion.sweepDefaultNotChange.json", "toolbox #823: an output in the default basket and not change (222's earlier insertion) is swept into a named basket, 200 (247)."),
        ("245-relinquishOutput.internalize.legacyDefault.json", "toolbox #823: the other earlier insertion into default leaves it by relinquishOutput, 200 (246)."),
        ("246-listOutputs.internalize.default.json", "toolbox #819, #823: default after the rows: the three unspent funded coins and the received payment (238); the two earlier insertions gone (244, 245)."),
        ("247-listOutputs.internalize.swept.json", "toolbox #823: the two outputs swept into the named basket (243, 244)."),
    ]
    .into_iter()
    .map(|(f, n)| (f.to_string(), n.to_string()))
    .collect()
}
