//! The owner's four commands over the per-origin grant rows (#414, the
//! design's L2 exit test, `docs/design/origin-grants.md` section (4)), run
//! as a user runs them on a throwaway wallet made by `init` with
//! `CHAINTRACKS_URL=off`: a grant listed, revoked, listed revoked, granted
//! again; pending rows seeded through the store's module and answered from
//! the CLI; the refused originator forms with their rules; the `--json`
//! keys in section (4)'s order; a revoke of no live row exits 1 naming the
//! originator. No door is called and nothing is broadcast.

use std::process::{Command, Output};

use bsv_wallet_cli::config::originator::{NAME, PARTS, SCHEME};
use bsv_wallet_cli::store::origin_grants::{self as grants, Asking, Scope};
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
    let path = dir.path().join("wallet.db");
    let storage = StorageSqlx::open(path.to_str().expect("utf-8 path"))
        .await
        .expect("open the store");
    let mut ids = Vec::new();
    for asking in asks {
        let pending = grants::open_or_join(&storage, asking, 1_793_577_600)
            .await
            .expect("a pending row");
        ids.push(pending.row.pending_id);
    }
    ids
}

fn wallet_call(originator: &str) -> Asking<'_> {
    Asking {
        originator,
        scope: Scope::Wallet,
        call: "/listOutputs",
        amount: None,
        renewal: false,
    }
}

#[test]
fn a_grant_is_listed_revoked_listed_revoked_and_granted_again() {
    let dir = made();
    let granted = said(&dir, &["origin-grant", "App.Example:8443"]);
    assert!(
        granted.starts_with("granted  app.example  wallet    since "),
        "{granted}"
    );
    assert!(
        granted.ends_with("  expires never  source owner\n"),
        "{granted}"
    );
    assert_eq!(said(&dir, &["origins"]), granted);

    let revoked = said(&dir, &["origin-revoke", "app.example"]);
    assert!(
        revoked.starts_with("revoked  app.example  wallet    at "),
        "{revoked}"
    );
    assert_eq!(revoked.lines().count(), 1, "{revoked}");
    assert_eq!(said(&dir, &["origins", "--all"]), revoked);

    let none = refused(&dir, &["origin-revoke", "app.example:9000"]);
    assert!(
        none.contains("app.example holds no live grant row: nothing to revoke"),
        "{none}"
    );
    let scoped = refused(
        &dir,
        &["origin-revoke", "app.example", "--scope", "spending"],
    );
    assert!(
        scoped.contains("app.example holds no live grant row of the spending scope"),
        "{scoped}"
    );

    let again = said(&dir, &["origin-grant", "app.example", "--expires", "3600"]);
    assert!(
        again.starts_with("granted  app.example  wallet    "),
        "{again}"
    );
    assert!(!again.contains("expires never"), "{again}");
    let spending = said(
        &dir,
        &[
            "origin-grant",
            "app.example",
            "--scope",
            "spending",
            "--amount",
            "60000",
        ],
    );
    assert!(
        spending.contains("granted  app.example  spending  60000/month (0 spent 20"),
        "{spending}"
    );
    let both = said(&dir, &["origin-revoke", "app.example"]);
    assert_eq!(both.lines().count(), 2, "{both}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_rows_are_answered_from_the_cli() {
    let dir = made();
    let spend = Asking {
        originator: "shop.example",
        scope: Scope::Spending,
        call: "/createAction",
        amount: Some(2500),
        renewal: false,
    };
    let ids = seed(
        &dir,
        &[
            wallet_call("new.example:5173"),
            wallet_call("deny.example"),
            spend,
        ],
    )
    .await;
    let listed = said(&dir, &["origins"]);
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 3, "{listed}");
    let first = format!(
        "pending  #{}  new.example:5173  wallet  /listOutputs  seen ",
        ids[0]
    );
    assert!(lines[0].starts_with(&first), "{listed}");
    assert!(lines[2].contains("  shop.example  spending  /createAction  2500 sat  seen "));

    let answered = said(&dir, &["origin-grant", &format!("#{}", ids[0])]);
    let answered: Vec<&str> = answered.lines().collect();
    assert!(answered[0].starts_with("granted  new.example  wallet    since "));
    assert!(answered[0].ends_with("  source owner"), "{answered:?}");
    assert!(answered[1].starts_with(&format!("answered  #{}  new.example:5173", ids[0])));
    assert!(answered[1].contains("  grant at "), "{answered:?}");

    let denied = said(&dir, &["origin-deny", &format!("#{}", ids[1])]);
    assert!(denied.starts_with(&format!("answered  #{}  deny.example", ids[1])));
    assert!(denied.contains("  deny at "), "{denied}");
    let twice = refused(&dir, &["origin-deny", &ids[1].to_string()]);
    assert!(twice.contains("was already answered deny at"), "{twice}");

    let amountless = refused(&dir, &["origin-grant", &format!("#{}", ids[2])]);
    assert!(amountless.contains("the spending scope needs --amount SATS"));
    let once = said(&dir, &["origin-grant", &format!("#{}", ids[2]), "--once"]);
    assert!(once.starts_with(&format!("answered  #{}  shop.example", ids[2])));
    assert!(once.contains("  once at "), "{once}");

    let open = said(&dir, &["origins"]);
    assert_eq!(open.lines().count(), 1, "only new.example's grant: {open}");
    let all = said(&dir, &["origins", "--all"]);
    assert_eq!(all.lines().count(), 4, "{all}");
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
        let e = refused(&dir, &["origin-grant", name]);
        assert!(
            e.contains(&format!("{name:?} is not {rule}")),
            "{name}: {e}"
        );
        let e = refused(&dir, &["origin-revoke", name]);
        assert!(e.contains(rule), "{name}: {e}");
    }
    // An IP address passes PR 419's rule, the named divergence from the
    // reference (the design's W9): it is granted as its own key.
    let ip = said(&dir, &["origin-grant", "127.0.0.1:8080"]);
    assert!(ip.starts_with("granted  127.0.0.1  wallet    "), "{ip}");
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
        "scope",
        "amount",
        "spentThisMonth",
        "grantedAt",
        "expiry",
        "revokedAt",
        "source",
    ];
    let pending = [
        "pendingId",
        "originator",
        "rawOriginator",
        "scope",
        "call",
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
    keys
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_json_shape_is_section_fours() {
    let dir = made();
    said(&dir, &["origin-grant", "app.example:8443"]);
    let renewal = Asking {
        renewal: true,
        ..wallet_call("new.example:5173")
    };
    let ids = seed(&dir, &[renewal]).await;
    let out = said(&dir, &["--json", "origins"]);
    assert_eq!(keys_in_order(&out), section_four_keys(), "{out}");

    let v: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    let granted_at = v["grants"][0]["grantedAt"].as_i64().expect("unix seconds");
    assert_eq!(
        v,
        serde_json::json!({
            "grants": [{
                "originator": "app.example",
                "rawOriginator": "app.example:8443",
                "scope": "wallet",
                "amount": null,
                "spentThisMonth": null,
                "grantedAt": granted_at,
                "expiry": 0,
                "revokedAt": null,
                "source": "owner"
            }],
            "pending": [{
                "pendingId": ids[0],
                "originator": "new.example",
                "rawOriginator": "new.example:5173",
                "scope": "wallet",
                "call": "/listOutputs",
                "amount": null,
                "renewal": true,
                "firstSeen": 1_793_577_600,
                "answeredAt": null,
                "answer": null
            }]
        })
    );

    let revoked = said(&dir, &["--json", "origin-revoke", "app.example"]);
    let v: serde_json::Value = serde_json::from_str(&revoked).expect("JSON");
    assert!(v["grants"][0]["revokedAt"].is_i64(), "{revoked}");
    assert_eq!(v["pending"], serde_json::json!([]));
}
