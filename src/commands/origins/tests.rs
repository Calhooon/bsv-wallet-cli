//! The lines of `origins` against section (4)'s examples, and
//! `origin-grant`'s arguments read before the wallet opens.

use super::*;

/// 2026-11-02T00:00:00Z.
const DAY: i64 = 1_793_577_600;

fn row(originator: &str, scope: Scope, source: Source) -> GrantRow {
    GrantRow {
        grant_id: 1,
        originator: originator.to_string(),
        raw_originator: originator.to_string(),
        scope,
        amount: None,
        granted_at: DAY + 70_260,
        expiry: 0,
        revoked_at: None,
        source,
    }
}

fn asked(id: i64, raw: &str, scope: Scope, call: &str) -> PendingRow {
    PendingRow {
        pending_id: id,
        originator: raw.split(':').next().unwrap_or(raw).to_string(),
        raw_originator: raw.to_string(),
        scope,
        call: call.to_string(),
        amount: None,
        renewal: false,
        first_seen: DAY + 86_400 + 33_120,
        answered_at: None,
        answer: None,
    }
}

#[test]
fn the_grant_lines_are_section_fours() {
    let env = row("app.example", Scope::Wallet, Source::Env);
    assert_eq!(
        grant_line(&env, None, DAY),
        "granted  app.example  wallet    since 2026-11-02T19:31Z  expires never  source env"
    );
    let spending = GrantRow {
        amount: Some(60_000),
        ..row("app.example", Scope::Spending, Source::Owner)
    };
    assert_eq!(
        grant_line(&spending, Some(12_000), DAY),
        "granted  app.example  spending  60000/month (12000 spent 2026-11)  expires never  \
         source owner"
    );
    let revoked = GrantRow {
        revoked_at: Some(DAY + 86_400 + 28_800),
        ..row("old.example", Scope::Wallet, Source::Owner)
    };
    assert_eq!(
        grant_line(&revoked, None, DAY),
        "revoked  old.example  wallet    at 2026-11-03T08:00Z"
    );
}

#[test]
fn an_expired_row_and_an_env_spending_row_say_so() {
    let dated = GrantRow {
        expiry: DAY + 60,
        ..row("app.example", Scope::Wallet, Source::Owner)
    };
    assert!(grant_line(&dated, None, DAY).contains("expires 2026-11-02T00:01Z"));
    assert!(grant_line(&dated, None, DAY + 61).starts_with("expired  app.example"));
    let env = row("app.example", Scope::Spending, Source::Env);
    assert!(grant_line(&env, Some(0), DAY).contains("spending  no ceiling (0 spent 2026-11)"));
}

#[test]
fn the_pending_lines_are_section_fours_with_their_seen_time() {
    let open = asked(7, "new.example:5173", Scope::Wallet, "/listOutputs");
    assert_eq!(
        pending_line(&open),
        "pending  #7  new.example:5173  wallet  /listOutputs  seen 2026-11-03T09:12Z"
    );
    let spend = PendingRow {
        amount: Some(2500),
        renewal: true,
        ..asked(8, "app.example", Scope::Spending, "/createAction")
    };
    assert_eq!(
        pending_line(&spend),
        "pending  #8  app.example  spending  /createAction  2500 sat  renewal  \
         seen 2026-11-03T09:12Z"
    );
    let denied = PendingRow {
        answered_at: Some(DAY + 86_400 + 33_180),
        answer: Some(Answer::Deny),
        ..open
    };
    assert!(pending_line(&denied).starts_with("answered  #7"));
    assert!(pending_line(&denied).ends_with("  deny at 2026-11-03T09:13Z"));
}

#[test]
fn origin_grant_reads_a_pending_id_or_an_originator() {
    let answer = Grant::read("#7", None, None, None, true).unwrap();
    assert_eq!(
        answer,
        Grant::Answer {
            pending_id: 7,
            once: true,
            amount: None,
            expires: None
        }
    );
    assert_eq!(Grant::read("7", None, None, None, false).unwrap(), {
        Grant::Answer {
            pending_id: 7,
            once: false,
            amount: None,
            expires: None,
        }
    });
    let direct = Grant::read("app.example:8443", None, None, Some("3600"), false).unwrap();
    assert_eq!(
        direct,
        Grant::Direct {
            originator: "app.example:8443".to_string(),
            scope: Scope::Wallet,
            amount: None,
            expires: Some(3600),
        }
    );
    assert_eq!(pending_id("#12").unwrap(), 12);
    assert!(pending_id("app.example").is_err());
}

#[test]
fn origin_grant_refuses_its_flags_out_of_place_by_name() {
    let refused = |r: Result<Grant>| r.unwrap_err().to_string();
    let spending = Grant::read("app.example", Some("spending"), None, None, false);
    assert_eq!(refused(spending), AMOUNT_NEEDED);
    let once = Grant::read("app.example", None, None, None, true);
    assert!(refused(once).starts_with("--once answers a pending spending request"));
    let scoped = Grant::read("#7", Some("wallet"), None, None, false);
    assert!(refused(scoped).contains("--scope is for a direct grant"));
    for word in ["0", "-1", "soon"] {
        let e = refused(Grant::read("app.example", None, None, Some(word), false));
        assert!(e.starts_with("--expires takes seconds"), "{word}: {e}");
    }
    assert_eq!(seconds("never").unwrap(), None);
}
