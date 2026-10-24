//! The configuration's reading of each setting: the defaults, the switches'
//! words, the refusals, the redaction and the sources.

use super::*;

fn config(pairs: &[(&str, &str)]) -> Config {
    Config::from_pairs(pairs).unwrap()
}

fn refusal(pairs: &[(&str, &str)]) -> ConfigError {
    Config::from_pairs(pairs).unwrap_err()
}

#[test]
fn an_empty_environment_is_every_default_of_the_wallet_settings() {
    let c = Config::default();
    assert_eq!(c.wallet.db, "wallet.db");
    assert_eq!(c.chain(), Chain::Main);
    assert_eq!(c.wallet.port, 3322);
    assert!(c.wallet.root_key.is_none());
    assert!(c.services.chaintracks_url.is_none());
    assert!(
        c.services.header_service().is_err(),
        "no default header service"
    );
    assert!(!c.services.arcade());
    assert_eq!(c.host.spend_depth, 1);
    assert!(c.host.spend_own_change);
    assert_eq!(c.host.tracker_age_secs, 600);
    assert_eq!(c.host.tracker_max_asks, 20);
}

#[test]
fn an_empty_environment_is_every_default_of_the_served_settings() {
    let c = Config::default();
    assert!(c.reconcile.enabled);
    assert_eq!(c.reconcile.interval_secs, 60);
    assert_eq!(c.reconcile.absence_minutes, 30);
    assert!(!c.verify.skip);
    assert_eq!(c.verify.attempts, 14);
    assert_eq!(c.verify.delay_ms, 2500);
    assert_eq!(c.server.bind_addr, IpAddr::from([127, 0, 0, 1]));
    assert!(c.server.auth_token.is_none());
    assert_eq!(c.daemon.check_proofs_interval_secs, None);
    assert_eq!(c.daemon.min_utxos, 3);
    assert!(c.daemon.reconcile_abandoned);
    assert_eq!(c.daemon.reconcile_abandoned_min_age_secs, 3600);
    assert_eq!(c.daemon.relay_url, None);
    assert_eq!(c.daemon.relay_poll_secs, 10);
}

#[test]
fn a_malformed_value_is_refused_naming_the_variable_and_the_shape() {
    for (name, value, shape) in [
        ("SPEND_DEPTH", "two", "a block depth"),
        ("TRACKER_AGE_SECS", "-1", "a number of seconds"),
        ("MIN_UTXOS", "3.5", "a whole number"),
        ("BIND_ADDR", "localhost", "an IP address"),
        ("CHAINTRACKS_URL", "headers.example", "an http or https URL"),
        ("ARC_URL", "ftp://arc.example", "an http or https URL"),
        ("WOC_BASE", "not a url", "an http or https URL"),
        ("BANANABLOCKS_BASE", "nope", "an http or https URL"),
        ("RELAY_URL", "relay", "an http or https URL"),
        (
            "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
            "soon",
            "milliseconds",
        ),
    ] {
        let e = refusal(&[(name, value)]);
        assert_eq!(e.variable, name);
        let text = e.to_string();
        assert!(text.starts_with(name), "{text}");
        assert!(text.contains(value), "{text}");
        assert!(text.contains(shape), "{text}");
    }
}

#[test]
fn unset_or_blank_is_the_default_and_numbers_are_trimmed() {
    assert_eq!(config(&[("SPEND_DEPTH", "  ")]).host.spend_depth, 1);
    assert_eq!(config(&[("SPEND_DEPTH", " 3 ")]).host.spend_depth, 3);
    assert_eq!(
        config(&[("SPEND_DEPTH", "0")]).host.spend_depth,
        1,
        "R3's floor"
    );
    assert_eq!(
        config(&[("TRACKER_AGE_SECS", "0")]).host.tracker_age_secs,
        1
    );
    assert_eq!(
        config(&[("BROADCAST_RECONCILE_INTERVAL_SECS", "1")])
            .reconcile
            .interval_secs,
        5
    );
    assert_eq!(
        config(&[("BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS", "0")])
            .verify
            .attempts,
        14
    );
    assert_eq!(
        config(&[("MONITOR_CHECK_PROOFS_INTERVAL_SECS", "0")])
            .daemon
            .check_proofs_interval_secs,
        None
    );
    assert_eq!(
        config(&[("BIND_ADDR", "0.0.0.0")]).server.bind_addr,
        IpAddr::from([0, 0, 0, 0])
    );
}

#[test]
fn each_switch_keeps_its_words() {
    let own_change = |v| config(&[("SPEND_OWN_CHANGE", v)]).host.spend_own_change;
    for off in ["0", "false", "NO", " off "] {
        assert!(!own_change(off), "{off}");
    }
    assert!(own_change("anything"));
    let reconcile = |v| config(&[("BROADCAST_RECONCILE", v)]).reconcile.enabled;
    assert!(!reconcile("0") && !reconcile("FALSE"));
    assert!(reconcile("no") && reconcile(" 0"));
    let skip = |v| {
        config(&[("BSV_WALLET_SKIP_BROADCAST_VERIFY", v)])
            .verify
            .skip
    };
    assert!(skip("1") && skip(" On ") && !skip("2"));
    let break_glass = |v| {
        config(&[("BREAK_GLASS_EXPLORER_HEADERS", v)])
            .services
            .break_glass_explorer_headers
    };
    assert!(break_glass("yes") && !break_glass("on"));
    assert!(config(&[("ARC_MODE", "Arcade")]).services.arcade());
    assert!(config(&[("ARCADE", " true ")]).services.arcade());
    assert!(!config(&[("ARC_MODE", "arc"), ("ARCADE", "0")])
        .services
        .arcade());
}

#[test]
fn the_services_keep_their_empty_meanings() {
    let s = config(&[("BANANABLOCKS_BASE", ""), ("ARC_GORILLAPOOL_URL", " ")]).services;
    assert_eq!(
        s.explorers.bananablocks_base.as_deref(),
        Some(""),
        "empty is off"
    );
    assert_eq!(
        s.arc_gorillapool_url.as_deref(),
        Some(""),
        "empty leaves it out"
    );
    let s = config(&[
        ("WOC_API_KEY", "  "),
        ("TAAL_API_KEY", ""),
        ("CALLBACK_TOKEN", ""),
    ])
    .services;
    assert!(
        s.explorers.woc_api_key.is_none() && s.taal_api_key.is_none() && s.callback_token.is_none()
    );
    let s = config(&[("CHAINTRACKS_URL", " OFF ")]).services;
    assert_eq!(s.header_service().unwrap(), None);
    let s = config(&[("CHAINTRACKS_URL", "https://headers.example ")]).services;
    assert_eq!(
        s.header_service().unwrap().as_deref(),
        Some("https://headers.example")
    );
}

#[test]
fn a_secret_prints_as_set_everywhere() {
    let c = config(&[
        ("ROOT_KEY", "00ff-root"),
        ("AUTH_TOKEN", "tok-auth"),
        ("CALLBACK_TOKEN", "tok-callback"),
        ("TAAL_API_KEY", "key-taal"),
        ("MAIN_TAAL_API_KEY", "key-main-taal"),
        ("WOC_API_KEY", "key-woc"),
        ("BITAILS_API_KEY", "key-bitails"),
    ]);
    let printed = format!("{c:?} {} {:?}", c.log_line(), c.knobs());
    for secret in [
        "00ff-root",
        "tok-auth",
        "tok-callback",
        "key-taal",
        "key-main-taal",
        "key-woc",
        "key-bitails",
    ] {
        assert!(!printed.contains(secret), "{secret} printed");
    }
    assert!(c.log_line().contains("AUTH_TOKEN=<set>"));
    assert_eq!(c.wallet.root_key().unwrap(), "00ff-root");
    assert_eq!(format!("{}", Secret::new("x")), "<set>");
}

/// Every environment variable `bsv-wallet` reads, in the knob list's order.
const VARIABLES: &[&str] = &[
    "ROOT_KEY",
    "RUST_LOG",
    "CHAINTRACKS_URL",
    "BREAK_GLASS_EXPLORER_HEADERS",
    "ARC_URL",
    "ARC_MODE",
    "ARCADE",
    "CALLBACK_TOKEN",
    "PUBLIC_CALLBACK_URL",
    "TAAL_API_KEY",
    "MAIN_TAAL_API_KEY",
    "WOC_API_KEY",
    "BITAILS_API_KEY",
    "BANANABLOCKS_BASE",
    "WOC_BASE",
    "BITAILS_BASE",
    "ARC_GORILLAPOOL_URL",
    "SPEND_DEPTH",
    "SPEND_OWN_CHANGE",
    "TRACKER_AGE_SECS",
    "TRACKER_MAX_ASKS",
    "BROADCAST_RECONCILE",
    "BROADCAST_RECONCILE_INTERVAL_SECS",
    "BROADCAST_ABSENCE_MINUTES",
    "BSV_WALLET_SKIP_BROADCAST_VERIFY",
    "BSV_WALLET_BROADCAST_VERIFY_ATTEMPTS",
    "BSV_WALLET_BROADCAST_VERIFY_DELAY_MS",
    "BIND_ADDR",
    "AUTH_TOKEN",
    "TLS_CERT_PATH",
    "TLS_KEY_PATH",
    "MONITOR_CHECK_PROOFS_INTERVAL_SECS",
    "MIN_UTXOS",
    "RECONCILE_ABANDONED",
    "RECONCILE_ABANDONED_MIN_AGE_SECS",
    "RELAY_URL",
    "RELAY_POLL_SECS",
];

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn every_variable_is_a_knob_with_its_source() {
    let flags = Flags {
        port: 4000,
        given: vec!["--port"],
        ..Flags::default()
    };
    let c = Config::from_vars(&flags, vars(&[("MIN_UTXOS", "7"), ("UNRELATED", "x")])).unwrap();
    let knobs = c.knobs();
    let names: Vec<_> = knobs.iter().map(|k| k.name).collect();
    assert_eq!(&names[..3], FLAG_NAMES);
    assert_eq!(&names[3..], VARIABLES);
    let of = |name| knobs.iter().find(|k| k.name == name).unwrap();
    assert_eq!(
        (of("--port").value.as_str(), of("--port").source),
        ("4000", Source::Flag)
    );
    assert_eq!(of("--db").source, Source::Default);
    assert_eq!(
        (of("MIN_UTXOS").value.as_str(), of("MIN_UTXOS").source),
        ("7", Source::Env)
    );
    assert_eq!(of("RELAY_POLL_SECS").source, Source::Default);
    assert_eq!(c.raw("MIN_UTXOS"), Some("7"));
    assert_eq!(c.log_line().split(' ').count(), knobs.len());
    assert!(
        !format!("{c:?}").contains("UNRELATED"),
        "the snapshot's names stay out of Debug"
    );
}

#[test]
fn a_missing_root_key_is_the_refusal_it_always_was() {
    let e = Config::default().wallet.root_key().unwrap_err();
    assert_eq!(
        e.to_string(),
        "ROOT_KEY not set. Run `bsv-wallet init` first."
    );
    assert_eq!(
        format!("{e:#}"),
        "ROOT_KEY not set. Run `bsv-wallet init` first.: environment variable not found"
    );
}

#[test]
fn the_relay_reads_its_own_settings() {
    let r = RelaySettings::from_vars([]).unwrap();
    assert_eq!((r.db.as_str(), r.port), ("relay.db", 3390));
    assert_eq!(r.bind_addr, IpAddr::from([0, 0, 0, 0]));
    assert!(r.admin_token.is_none() && !r.require_register);
    let r = RelaySettings::from_vars(vars(&[
        ("RELAY_ADMIN_TOKEN", "admin"),
        ("RELAY_REQUIRE_REGISTER", "TRUE"),
    ]))
    .unwrap();
    assert_eq!(
        r.admin_token.map(|t| t.expose().to_string()).as_deref(),
        Some("admin")
    );
    assert!(r.require_register);
    let e = RelaySettings::from_vars(vars(&[("RELAY_PORT", "99999")])).unwrap_err();
    assert_eq!(e.variable, "RELAY_PORT");
}
