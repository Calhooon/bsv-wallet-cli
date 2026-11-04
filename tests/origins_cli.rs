//! The owner's four commands over the per-origin grant rows (#414, the
//! design's L2 exit test, `docs/design/origin-grants.md` section (4)), run
//! as a user runs them on a throwaway wallet made by `init` with
//! `CHAINTRACKS_URL=off`: a power of each kind granted, listed, revoked by
//! its key, by its kind and all at once, and granted again; pending rows
//! of each kind seeded through the store's module and answered from the
//! CLI; a key off its canonical form refused naming it; the refused
//! originator forms with their rules; the `--json` keys in section (4)'s
//! order with the probe record (#539) in its own section, which a revoke
//! leaves; a revoke of no live row exits 1 naming the originator and the
//! power. No door is called and nothing is broadcast.

use std::process::{Command, Output};

use bsv_wallet_cli::config::originator::{NAME, PARTS, SCHEME};
use bsv_wallet_cli::store::origin_grants::{self as grants, Asking, Power, Probe};
use bsv_wallet_toolbox::StorageSqlx;
use tempfile::TempDir;

/// The variables a run must not take from the shell.
const UNSET: [&str; 5] = [
    "ROOT_KEY",
    "ARC_MODE",
    "ARCADE",
    "ARC_URL",
    "ALLOWED_ORIGINS",
];
/// The variables a run is given: no header service, and any request the
/// run would make goes to a closed local port.
const SET: [(&str, &str); 4] = [
    ("CHAINTRACKS_URL", "off"),
    ("HTTPS_PROXY", "http://127.0.0.1:9"),
    ("HTTP_PROXY", "http://127.0.0.1:9"),
    ("RUST_LOG", "warn"),
];

/// A wallet made by `init` in a temporary directory.
fn made() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let init = run(&dir, &["init"]);
    assert!(init.status.success(), "init: {}", text(&init.stderr));
    dir
}

fn run(dir: &TempDir, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bsv-wallet"));
    cmd.current_dir(dir.path())
        .envs(SET)
        .args(["--db", "wallet.db"])
        .args(args);
    for name in UNSET {
        cmd.env_remove(name);
    }
    cmd.output().expect("run the command")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The command's stdout, asserting it exits 0.
fn said(dir: &TempDir, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

/// The command's stderr, asserting it exits 1 and prints nothing.
fn refused(dir: &TempDir, args: &[&str]) -> String {
    let out = run(dir, args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{args:?}: {}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stdout), "", "{args:?}");
    text(&out.stderr)
}

/// Pending rows written through the store's module, as a held call
/// writes them: their ids in order.
async fn seed(dir: &TempDir, asks: &[Asking<'_>]) -> Vec<i64> {
    let storage = opened(dir).await;
    let mut ids = Vec::new();
    for asking in asks {
        let pending = grants::open_or_join(&storage, asking, 1_793_577_600)
            .await
            .expect("a pending row");
        ids.push(pending.row.pending_id);
    }
    ids
}

async fn opened(dir: &TempDir) -> StorageSqlx {
    let path = dir.path().join("wallet.db");
    StorageSqlx::open(path.to_str().expect("utf-8 path"))
        .await
        .expect("open the store")
}

/// The probe record's rows, as the gate's writer writes them: `getVersion`
/// twice from `new.example`.
async fn probed(dir: &TempDir) {
    let probe = |at| Probe {
        originator: "new.example:5173".to_string(),
        door: "getVersion".to_string(),
        power: None,
        at,
    };
    let seen = [probe(1_793_577_600), probe(1_793_577_660)];
    grants::probes::record(&opened(dir).await, &seen)
        .await
        .expect("the record");
}

/// The generator's public key, a verifier; a certificate type.
const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const TYPE: &str = "AGfk/WrT1eBDXpz3mcw386Zww2HmqcIn3uY6x4Af1eo=";

/// A power of each kind as `origin-grant` takes it: its kind, its key and
/// the flags beyond them; with the line's middle `origins` prints.
fn every_kind() -> Vec<(&'static str, Option<String>, Vec<&'static str>)> {
    vec![
        (
            "protocol",
            Some(r#"[false,2,"todo list","self"]"#.to_string()),
            vec![],
        ),
        (
            "protocol",
            Some(r#"[false,1,"action label invoices",""]"#.to_string()),
            vec![],
        ),
        ("basket", Some(r#"["todo items"]"#.to_string()), vec![]),
        (
            "certificate",
            Some(format!(r#"[false,"{TYPE}","{G}",["email","name"]]"#)),
            vec![],
        ),
        ("spending", None, vec!["--amount", "60000"]),
    ]
}

/// `origin-grant <originator> --kind K [--key KEY] [flags]`.
fn grant_args<'a>(
    originator: &'a str,
    kind: &'a str,
    key: Option<&'a str>,
    flags: &[&'a str],
) -> Vec<&'a str> {
    let mut args = vec!["origin-grant", originator, "--kind", kind];
    if let Some(key) = key {
        args.extend(["--key", key]);
    }
    args.extend(flags);
    args
}

/// A wallet holding a grant of each kind to `app.example:8443`, and the
/// line `origin-grant` printed for each.
fn granted_each_kind() -> (TempDir, Vec<String>) {
    let dir = made();
    let mut lines = Vec::new();
    for (kind, key, flags) in every_kind() {
        let args = grant_args("App.Example:8443", kind, key.as_deref(), &flags);
        let granted = said(&dir, &args);
        let key = key.unwrap_or_else(|| "[]".into());
        let head = format!("granted  app.example  {kind:<11}  {key}  ");
        assert!(granted.starts_with(&head), "{granted}");
        lines.push(granted);
    }
    (dir, lines)
}

#[test]
fn a_power_of_each_kind_is_granted_and_listed() {
    let (dir, lines) = granted_each_kind();
    let listed = said(&dir, &["origins"]);
    assert_eq!(listed.lines().count(), 5, "{listed}");
    for line in &lines {
        assert!(listed.contains(line.trim_end()), "{line} in {listed}");
    }
    let todo = r#"["todo items"]"#;
    let again = grant_args("app.example", "basket", Some(todo), &["--expires", "3600"]);
    let again = said(&dir, &again);
    assert!(
        again.starts_with("granted  app.example  basket       "),
        "{again}"
    );
    assert!(!again.contains("expires never"), "{again}");
    assert_eq!(
        said(&dir, &["origins"]).lines().count(),
        5,
        "one row per power"
    );
}

#[test]
fn a_revoke_reaches_one_power_a_kind_or_every_row() {
    let (dir, _) = granted_each_kind();
    let todo = r#"["todo items"]"#;
    let one = [
        "origin-revoke",
        "app.example",
        "--kind",
        "basket",
        "--key",
        todo,
    ];
    let revoked = said(&dir, &one);
    let head = format!("revoked  app.example  basket       {todo}  since ");
    assert!(revoked.starts_with(&head), "{revoked}");
    assert!(
        revoked.contains("  expires never  revoked at "),
        "{revoked}"
    );
    assert_eq!(revoked.lines().count(), 1, "{revoked}");
    let none = refused(&dir, &one);
    let why = format!("app.example holds no live grant row for basket {todo}: nothing to revoke");
    assert!(none.contains(&why), "{none}");
    let protocols = said(
        &dir,
        &["origin-revoke", "app.example:9000", "--kind", "protocol"],
    );
    assert_eq!(
        protocols.lines().count(),
        2,
        "the protocol and the label: {protocols}"
    );
    let none = refused(
        &dir,
        &["origin-revoke", "app.example", "--kind", "protocol"],
    );
    assert!(
        none.contains("holds no live grant row of the protocol kind"),
        "{none}"
    );
    let rest = said(&dir, &["origin-revoke", "app.example"]);
    assert_eq!(
        rest.lines().count(),
        2,
        "the certificate and spending: {rest}"
    );
    let none = refused(&dir, &["origin-revoke", "app.example"]);
    let why = "app.example holds no live grant row: nothing to revoke";
    assert!(none.contains(why), "{none}");
    assert_eq!(said(&dir, &["origins", "--all"]).lines().count(), 5);
}

#[test]
fn a_key_off_its_rules_is_refused_by_name() {
    let dir = made();
    for (kind, key, words) in [
        ("basket", r#"["Todo Items"]"#, r#"write it ["todo items"]"#),
        (
            "protocol",
            r#"[false, 2, "todo list", "self"]"#,
            r#"write it [false,2,"todo list","self"]"#,
        ),
        (
            "protocol",
            r#"[false,1,"todo list","self"]"#,
            r#"write it [false,1,"todo list",""]"#,
        ),
        (
            "protocol",
            r#"[false,0,"todo list",""]"#,
            "security level 0",
        ),
        (
            "basket",
            r#"["default"]"#,
            "Basket \u{201c}default\u{201d} is admin-only.",
        ),
        ("basket", "todo", "a basket key is"),
    ] {
        let e = refused(&dir, &grant_args("app.example", kind, Some(key), &[]));
        assert!(e.contains(words), "{kind} {key}: {e}");
    }
    let e = refused(&dir, &["origin-grant", "app.example"]);
    assert!(e.contains("a direct grant names its power: --kind"), "{e}");
    let e = refused(&dir, &["origin-grant", "app.example", "--kind", "basket"]);
    assert!(e.contains("a basket grant names its key"), "{e}");
    let e = refused(&dir, &grant_args("app.example", "spending", None, &[]));
    assert!(e.contains("the spending kind needs --amount SATS"), "{e}");
    // clap refuses a key without its kind, and the withdrawn scope.
    for args in [
        &["origin-grant", "app.example", "--key", "[]"][..],
        &["origin-revoke", "app.example", "--key", "[]"][..],
        &["origin-grant", "app.example", "--kind", "wallet"][..],
        &["origin-grant", "app.example", "--scope", "wallet"][..],
    ] {
        let out = run(&dir, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert_eq!(said(&dir, &["origins"]), "");
}

/// Three held calls' rows on `dir`'s wallet: a basket listing from
/// `new.example:5173`, a protocol signing from `deny.example` and a spend
/// of 2500 from `shop.example`; their ids.
async fn three_pending(dir: &TempDir) -> Vec<i64> {
    let todo = Power::basket("todo items").expect("a basket");
    let deny = Power::protocol(false, 2, "todo list", None).expect("a protocol");
    let ask = |originator, power, usage, call| Asking {
        originator,
        power,
        usage,
        call,
        reason: None,
        amount: None,
        renewal: false,
    };
    let spend = Asking {
        reason: Some("Buy a coffee"),
        amount: Some(2500),
        ..ask("shop.example", &Power::Spending, None, "/createAction")
    };
    let asks = [
        ask("new.example:5173", &todo, Some("listing"), "/listOutputs"),
        ask("deny.example", &deny, Some("signing"), "/createSignature"),
        spend,
    ];
    seed(dir, &asks).await
}

/// A second spend of shop.example's, granted with `--amount`: the spending
/// row with its ceiling and this month's spend, and the answered row.
async fn a_spend_is_granted_its_ceiling(dir: &TempDir) {
    let again = Asking {
        originator: "shop.example",
        power: &Power::Spending,
        usage: None,
        call: "/createAction",
        reason: None,
        amount: Some(2500),
        renewal: false,
    };
    let spend = seed(dir, &[again]).await[0];
    let granted = said(
        dir,
        &["origin-grant", &format!("#{spend}"), "--amount", "60000"],
    );
    let granted: Vec<&str> = granted.lines().collect();
    assert!(
        granted[0].starts_with("granted  shop.example  spending     []  60000/month (0 spent "),
        "{granted:?}"
    );
    assert!(granted[1].contains("  grant at "), "{granted:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_rows_of_each_kind_are_answered_from_the_cli() {
    let dir = made();
    let ids = three_pending(&dir).await;
    let listed = said(&dir, &["origins"]);
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 3, "{listed}");
    let first = format!(
        r#"pending  #{}  new.example:5173  basket  ["todo items"]  list the basket "todo items"  /listOutputs  seen "#,
        ids[0]
    );
    assert!(lines[0].starts_with(&first), "{listed}");
    assert!(lines[1]
        .contains(r#"use protocol "todo list" (level 2) with counterparty self for signing"#));
    assert!(
        lines[2].contains(r#"  shop.example  spending  []  spend 2500 satoshis "Buy a coffee"  /createAction  2500 sat  seen "#),
        "{listed}"
    );

    let answered = said(&dir, &["origin-grant", &format!("#{}", ids[0])]);
    let answered: Vec<&str> = answered.lines().collect();
    assert!(answered[0].starts_with(r#"granted  new.example  basket       ["todo items"]  since "#));
    assert!(answered[1].starts_with(&format!("answered  #{}  new.example:5173", ids[0])));
    assert!(answered[1].contains("  grant at "), "{answered:?}");
    let once = refused(&dir, &["origin-grant", &format!("#{}", ids[1]), "--once"]);
    assert!(once.contains("asks for a protocol power; once answers a spending request alone"));

    let denied = said(&dir, &["origin-deny", &format!("#{}", ids[1])]);
    assert!(denied.starts_with(&format!("answered  #{}  deny.example", ids[1])));
    assert!(denied.contains("  deny at "), "{denied}");
    let twice = refused(&dir, &["origin-deny", &ids[1].to_string()]);
    assert!(twice.contains("was already answered deny at"), "{twice}");

    // L6: a spend's grant names its ceiling.
    let e = refused(&dir, &["origin-grant", &format!("#{}", ids[2])]);
    assert!(e.contains("the spending kind needs --amount SATS"), "{e}");
    let kinded = refused(
        &dir,
        &[
            "origin-grant",
            &format!("#{}", ids[2]),
            "--kind",
            "spending",
        ],
    );
    assert!(
        kinded.contains("a pending request names its own power"),
        "{kinded}"
    );
    let once = said(&dir, &["origin-grant", &format!("#{}", ids[2]), "--once"]);
    assert!(once.starts_with(&format!("answered  #{}  shop.example", ids[2])));
    assert!(once.contains("  once at "), "{once}");

    a_spend_is_granted_its_ceiling(&dir).await;

    let open = said(&dir, &["origins"]);
    assert_eq!(
        open.lines().count(),
        2,
        "new.example's and shop.example's grants: {open}"
    );
    let all = said(&dir, &["origins", "--all"]);
    assert_eq!(all.lines().count(), 6, "{all}");
}

/// #534: a row whose caller disconnected waits for its page inside the
/// window: listed as such (the line and `abandonedAt`), and the owner's
/// `origin-grant` and `origin-deny` refused naming the window's end, no
/// grant row written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_row_is_listed_and_its_answer_refused_from_the_cli() {
    let dir = made();
    let basket = Power::basket("todo items").expect("a basket");
    let [id] = seed(
        &dir,
        &[Asking {
            originator: "back.example",
            power: &basket,
            usage: Some("listing"),
            call: "/listOutputs",
            reason: None,
            amount: None,
            renewal: false,
        }],
    )
    .await[..] else {
        panic!("one row");
    };
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_secs(),
    )
    .expect("seconds");
    let storage = opened(&dir).await;
    let done = grants::abandon(&storage, "back.example", &[id], now)
        .await
        .expect("the mark");
    assert_eq!(done.marked.len(), 1);
    let end = grants::when_exact(now + bsv_wallet_cli::server::powers::ABANDON_WINDOW_SECS);

    let listed = said(&dir, &["origins"]);
    assert!(
        listed.trim_end().ends_with(&format!(
            "caller disconnected, withdrawn at {end} unless the page asks again"
        )),
        "{listed}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&said(&dir, &["--json", "origins"])).expect("json");
    assert_eq!(json["pending"][0]["abandonedAt"], now);
    let words = format!(
        "pending request #{id} is waiting for its page to reconnect; answer it when the page \
         asks again, or it is withdrawn at {end}"
    );
    for command in ["origin-grant", "origin-deny"] {
        let e = refused(&dir, &[command, &format!("#{id}")]);
        assert!(e.contains(&words), "{command}: {e}");
    }
    let rows = grants::listing(&storage, true, now).await.expect("rows");
    assert!(rows.grants.is_empty(), "no grant row");
    assert_eq!(rows.pending[0].answered_at, None);
}

#[test]
fn the_refused_originator_forms_name_their_rule() {
    let dir = made();
    for (name, rule) in [
        ("http://app.example", SCHEME),
        ("app.example/path", PARTS),
        ("user@app.example", PARTS),
        ("app_example!", NAME),
    ] {
        let e = refused(
            &dir,
            &grant_args(name, "spending", None, &["--amount", "1"]),
        );
        assert!(
            e.contains(&format!("{name:?} is not {rule}")),
            "{name}: {e}"
        );
        let e = refused(&dir, &["origin-revoke", name]);
        assert!(e.contains(rule), "{name}: {e}");
    }
    // An IP address passes PR 419's rule, the named divergence from the
    // reference (the design's W9): it is granted as its own key.
    let ip = said(
        &dir,
        &grant_args("127.0.0.1:8080", "basket", Some(r#"["x"]"#), &[]),
    );
    assert!(
        ip.starts_with(r#"granted  127.0.0.1  basket       ["x"]  "#),
        "{ip}"
    );
    assert_eq!(said(&dir, &["origins"]), ip);
}

/// The keys of every object in `json`, in the order the text holds them.
fn keys_in_order(json: &str) -> Vec<String> {
    json.lines()
        .filter_map(|l| l.trim().strip_prefix('"'))
        .filter_map(|l| l.split_once("\":").map(|(k, _)| k.to_string()))
        .collect()
}

/// Section (4)'s `--json` keys of one grant row and one pending row, in
/// its order.
fn section_four_keys() -> Vec<String> {
    let grant = [
        "originator",
        "rawOriginator",
        "kind",
        "key",
        "state",
        "amount",
        "spentThisMonth",
        "ceilingReached",
        "grantedAt",
        "expiry",
        "revokedAt",
    ];
    let pending = [
        "pendingId",
        "originator",
        "rawOriginator",
        "kind",
        "key",
        "usage",
        "asked",
        "call",
        "reason",
        "amount",
        "renewal",
        "firstSeen",
        "answeredAt",
        "answer",
    ];
    let mut keys = vec!["grants".to_string()];
    keys.extend(grant.iter().map(ToString::to_string));
    keys.push("pending".to_string());
    keys.extend(pending.iter().map(ToString::to_string));
    keys.push("probes".to_string());
    keys.extend(
        [
            "originator",
            "rawOriginator",
            "door",
            "power",
            "count",
            "firstSeen",
            "lastSeen",
        ]
        .map(ToString::to_string),
    );
    keys
}

/// The `--json` listing of [`the_json_shape_is_section_fours`]'s grant row
/// and its pending row `id`.
fn section_four_row(granted_at: i64, id: i64) -> serde_json::Value {
    let todo = r#"["todo items"]"#;
    serde_json::json!({
        "grants": [{
            "originator": "app.example",
            "rawOriginator": "app.example:8443",
            "kind": "basket",
            "key": todo,
            "state": "granted",
            "amount": null,
            "spentThisMonth": null,
            "ceilingReached": null,
            "grantedAt": granted_at,
            "expiry": 0,
            "revokedAt": null
        }],
        "pending": [{
            "pendingId": id,
            "originator": "new.example",
            "rawOriginator": "new.example:5173",
            "kind": "basket",
            "key": todo,
            "usage": "listing",
            "asked": "list the basket \"todo items\"",
            "call": "/listOutputs",
            "reason": null,
            "amount": null,
            "renewal": true,
            "firstSeen": 1_793_577_600,
            "answeredAt": null,
            "answer": null
        }],
        "probes": [{
            "originator": "new.example",
            "rawOriginator": "new.example:5173",
            "door": "getVersion",
            "power": null,
            "count": 2,
            "firstSeen": 1_793_577_600,
            "lastSeen": 1_793_577_660
        }]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_json_shape_is_section_fours() {
    let dir = made();
    let todo = r#"["todo items"]"#;
    said(
        &dir,
        &grant_args("app.example:8443", "basket", Some(todo), &[]),
    );
    let power = Power::basket("todo items").expect("a basket");
    let renewal = Asking {
        originator: "new.example:5173",
        power: &power,
        usage: Some("listing"),
        call: "/listOutputs",
        reason: None,
        amount: None,
        renewal: true,
    };
    let ids = seed(&dir, &[renewal]).await;
    probed(&dir).await;
    let out = said(&dir, &["--json", "origins"]);
    assert_eq!(keys_in_order(&out), section_four_keys(), "{out}");

    let v: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    let granted_at = v["grants"][0]["grantedAt"].as_i64().expect("unix seconds");
    assert_eq!(v, section_four_row(granted_at, ids[0]));

    let revoked = said(&dir, &["--json", "origin-revoke", "app.example"]);
    let v: serde_json::Value = serde_json::from_str(&revoked).expect("JSON");
    assert!(v["grants"][0]["revokedAt"].is_i64(), "{revoked}");
    assert_eq!(v["grants"][0]["state"], "revoked", "{revoked}");
    assert_eq!(v["pending"], serde_json::json!([]));
    assert!(v.get("probes").is_none(), "an answer carries no record");
    let probe = "probe  new.example  getVersion  2 times  first 2026-11-02T00:00Z  last \
                 2026-11-02T00:01Z";
    let lines = said(&dir, &["origins"]);
    assert_eq!(lines.lines().last(), Some(probe), "the revoke left it");
}
