//! The lines of `origins` against section (4)'s examples, and
//! `origin-grant`'s and `origin-revoke`'s arguments read before the
//! wallet opens.

use super::*;
use bsv_wallet_cli::store::origin_grants::GrantRow;

/// 2026-11-02T00:00:00Z.
const DAY: i64 = 1_793_577_600;

fn row(originator: &str, power: &Power) -> GrantRow {
    GrantRow {
        grant_id: 1,
        originator: originator.to_string(),
        raw_originator: originator.to_string(),
        kind: power.kind(),
        key: power.key(),
        amount: None,
        granted_at: DAY + 70_260,
        expiry: 0,
        revoked_at: None,
    }
}

fn asked(id: i64, raw: &str, power: &Power, usage: Option<&str>, call: &str) -> PendingRow {
    PendingRow {
        pending_id: id,
        originator: raw.split(':').next().unwrap_or(raw).to_string(),
        raw_originator: raw.to_string(),
        kind: power.kind(),
        key: power.key(),
        usage: usage.map(str::to_string),
        call: call.to_string(),
        reason: None,
        amount: None,
        renewal: false,
        first_seen: DAY + 86_400 + 33_120,
        answered_at: None,
        answer: None,
        abandoned_at: None,
    }
}

fn todo_list() -> Power {
    Power::protocol(false, 2, "todo list", None).unwrap()
}

fn todo_items() -> Power {
    Power::basket("todo items").unwrap()
}

/// `row`'s line read at `now`, with `spent` the month's tally.
fn line(row: GrantRow, spent: Option<i64>, now: i64) -> String {
    grant_line(&GrantShown::read(row, spent, now).unwrap())
}

#[test]
fn the_grant_lines_are_section_fours() {
    assert_eq!(
        line(row("app.example", &todo_list()), None, DAY),
        r#"granted  app.example  protocol     [false,2,"todo list","self"]  since 2026-11-02T19:31Z  expires never"#
    );
    assert_eq!(
        line(row("app.example", &todo_items()), None, DAY),
        r#"granted  app.example  basket       ["todo items"]  since 2026-11-02T19:31Z  expires never"#
    );
    let spending = GrantRow {
        amount: Some(60_000),
        ..row("app.example", &Power::Spending)
    };
    assert_eq!(
        line(spending, Some(12_000), DAY),
        "granted  app.example  spending     []  60000/month (12000 spent 2026-11)  since \
         2026-11-02T19:31Z  expires never"
    );
    let g = "03".to_string() + &"ab".repeat(32);
    let revoked = GrantRow {
        kind: Kind::Certificate,
        key: format!(r#"[false,"AGfk","{g}",["email"]]"#),
        revoked_at: Some(DAY + 86_400 + 28_800),
        ..row("old.example", &todo_items())
    };
    assert_eq!(
        line(revoked, None, DAY),
        format!(
            r#"revoked  old.example  certificate  [false,"AGfk","{g}",["email"]]  since 2026-11-02T19:31Z  expires never  revoked at 2026-11-03T08:00Z"#
        )
    );
}

#[test]
fn an_expired_row_says_so() {
    let dated = GrantRow {
        expiry: DAY + 60,
        ..row("app.example", &todo_items())
    };
    assert!(line(dated.clone(), None, DAY).starts_with("granted  app.example"));
    assert!(line(dated.clone(), None, DAY).ends_with("expires 2026-11-02T00:01Z"));
    assert!(line(dated.clone(), None, DAY + 60).starts_with("granted  app.example"));
    assert!(line(dated.clone(), None, DAY + 61).starts_with("expired  app.example"));
    let gone = GrantRow {
        revoked_at: Some(DAY + 30),
        ..dated
    };
    assert!(line(gone, None, DAY + 61).starts_with("revoked  app.example"));
}

#[test]
fn a_ceiling_the_month_has_reached_says_so() {
    let spending = |amount| GrantRow {
        amount,
        ..row("app.example", &Power::Spending)
    };
    let under = line(spending(Some(1_500)), Some(1_499), DAY);
    assert!(
        under.contains("  1500/month (1499 spent 2026-11)  "),
        "{under}"
    );
    let at = line(spending(Some(1_500)), Some(1_500), DAY);
    assert!(
        at.contains("  1500/month (1500 spent 2026-11, at the ceiling)  "),
        "{at}"
    );
    let none = line(spending(None), Some(9_000), DAY);
    assert!(
        none.contains("  no ceiling (9000 spent 2026-11)  "),
        "{none}"
    );
}

#[test]
fn the_pending_lines_are_section_fours_with_their_seen_time() {
    let power = todo_items();
    let open = asked(
        7,
        "new.example:5173",
        &power,
        Some("listing"),
        "/listOutputs",
    );
    assert_eq!(
        pending_line(&open, &open.asked(None)),
        r#"pending  #7  new.example:5173  basket  ["todo items"]  list the basket "todo items"  /listOutputs  seen 2026-11-03T09:12Z"#
    );
    let spend = PendingRow {
        amount: Some(2500),
        renewal: true,
        reason: Some("Buy a coffee".to_string()),
        first_seen: DAY + 86_400 + 33_180,
        ..asked(8, "app.example", &Power::Spending, None, "/createAction")
    };
    assert_eq!(
        pending_line(&spend, &spend.asked(None)),
        r#"pending  #8  app.example  spending  []  spend 2500 satoshis "Buy a coffee"  /createAction  2500 sat  renewal  seen 2026-11-03T09:13Z"#
    );
    assert_eq!(
        spend.asked(Some((60_000, 12_000))),
        "spend 2500 satoshis past this month's ceiling of 60000 (12000 spent)"
    );
    let denied = PendingRow {
        answered_at: Some(DAY + 86_400 + 33_180),
        answer: Some(Answer::Deny),
        ..open
    };
    let line = pending_line(&denied, "x");
    assert!(line.starts_with("answered  #7"), "{line}");
    assert!(line.ends_with("  deny at 2026-11-03T09:13Z"), "{line}");
}

#[test]
fn origin_grant_reads_a_pending_id_or_an_originator_and_a_power() {
    let answer = Grant::read("#7", (None, None), None, None, true).unwrap();
    assert_eq!(
        answer,
        Grant::Answer {
            pending_id: 7,
            once: true,
            amount: None,
            expires: None
        }
    );
    assert_eq!(
        Grant::read("7", (None, None), None, None, false).unwrap(),
        {
            Grant::Answer {
                pending_id: 7,
                once: false,
                amount: None,
                expires: None,
            }
        }
    );
    let key = r#"[false,2,"todo list","self"]"#;
    let direct = Grant::read(
        "app.example:8443",
        (Some("protocol"), Some(key)),
        None,
        Some("3600"),
        false,
    );
    assert_eq!(
        direct.unwrap(),
        Grant::Direct {
            originator: "app.example:8443".to_string(),
            power: todo_list(),
            amount: None,
            expires: Some(3600),
        }
    );
    let spending = Grant::read(
        "app.example",
        (Some("spending"), None),
        Some(60_000),
        None,
        false,
    );
    assert!(matches!(
        spending.unwrap(),
        Grant::Direct {
            power: Power::Spending,
            amount: Some(60_000),
            ..
        }
    ));
    assert_eq!(pending_id("#12").unwrap(), 12);
    assert!(pending_id("app.example").is_err());
}

#[test]
fn origin_grant_refuses_its_flags_out_of_place_by_name() {
    let refused = |r: Result<Grant>| r.unwrap_err().to_string();
    let spending = Grant::read("app.example", (Some("spending"), None), None, None, false);
    assert_eq!(refused(spending), AMOUNT_NEEDED);
    let kindless = Grant::read("app.example", (None, None), None, None, false);
    assert_eq!(refused(kindless), KIND_NEEDED);
    let keyless = Grant::read("app.example", (Some("basket"), None), None, None, false);
    assert!(refused(keyless).starts_with("a basket grant names its key"));
    let loose = Grant::read(
        "app.example",
        (Some("basket"), Some(r#"[ "x"]"#)),
        None,
        None,
        false,
    );
    assert_eq!(
        refused(loose),
        r#"the basket key [ "x"] is not in its canonical form; write it ["x"]"#
    );
    let once = Grant::read("app.example", (Some("spending"), None), None, None, true);
    assert!(refused(once).starts_with("--once answers a pending spending request"));
    let kinded = Grant::read("#7", (Some("basket"), None), None, None, false);
    assert!(refused(kinded).contains("--kind and --key are for a direct grant"));
    for word in ["0", "-1", "soon"] {
        let e = refused(Grant::read(
            "app.example",
            (None, None),
            None,
            Some(word),
            false,
        ));
        assert!(e.starts_with("--expires takes seconds"), "{word}: {e}");
    }
    assert_eq!(seconds("never").unwrap(), None);
}

#[test]
fn origin_revoke_reads_its_reach() {
    assert_eq!(reach((None, None)).unwrap(), Reach::Every);
    assert_eq!(
        reach((Some("basket"), None)).unwrap(),
        Reach::Kind(Kind::Basket)
    );
    let one = reach((Some("basket"), Some(r#"["todo items"]"#))).unwrap();
    assert_eq!(one, Reach::Power(todo_items()));
    let e = reach((Some("basket"), Some(r#"["Todo Items"]"#))).unwrap_err();
    assert!(e.to_string().contains(r#"write it ["todo items"]"#), "{e}");
    assert!(
        reach((Some("wallet"), None)).is_err(),
        "the withdrawn scope"
    );
}
