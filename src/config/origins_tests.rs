//! `ALLOWED_ORIGINS` read through the typed config (#411): the browser
//! gate's originators, or the list refused by name.

use super::tests::{config, refusal, row};
use super::*;

/// #411, the key by the owner's word of 2026-11-02: `ALLOWED_ORIGINS`
/// holds originators as the reference's `prepareOriginator` reads one (a
/// host with an optional port, no scheme), lowercased at read; unset or
/// blank admits none, and an entry in any other form refuses the list by
/// its rule at start, never stripped to its host.
#[test]
fn allowed_origins_are_originators_or_refused() {
    assert!(Config::default().server.allowed_origins.is_empty());
    assert!(config(&[("ALLOWED_ORIGINS", " ")])
        .server
        .allowed_origins
        .is_empty());
    let c = config(&[(
        "ALLOWED_ORIGINS",
        "localhost, App.Example:8443,,127.0.0.1,e2e-test,[::1]:3000",
    )]);
    assert_eq!(
        c.server.allowed_origins,
        [
            "localhost",
            "app.example:8443",
            "127.0.0.1",
            "e2e-test",
            "[::1]:3000"
        ]
    );
    assert_eq!(
        row(&c, "ALLOWED_ORIGINS").0,
        "localhost,app.example:8443,127.0.0.1,e2e-test,[::1]:3000"
    );
    assert_eq!(row(&Config::default(), "ALLOWED_ORIGINS").0, "none");
    for (entry, rule) in [
        ("http://localhost", "without a URL scheme"),
        ("HTTPS://APP.EXAMPLE:8443", "without a URL scheme"),
        ("ftp://localhost", "without a URL scheme"),
        ("localhost/", "a path"),
        ("localhost/app", "a path"),
        ("user@localhost", "credentials"),
        ("localhost?x=1", "a query"),
        ("localhost#x", "a fragment"),
        ("*", "a DNS hostname"),
        ("*.example", "a DNS hostname"),
        ("app_example", "a DNS hostname"),
        ("-app.example", "a DNS hostname"),
        ("localhost:", "a DNS hostname"),
        ("localhost:99999", "a DNS hostname"),
        ("localhost:port", "a DNS hostname"),
    ] {
        let e = refusal(&[("ALLOWED_ORIGINS", &format!("localhost,{entry}"))]);
        assert_eq!(e.variable, "ALLOWED_ORIGINS", "{entry}");
        assert_eq!(e.value, entry);
        let shown = e.to_string();
        assert!(
            shown.contains(rule) && shown.contains("prepareOriginator"),
            "{entry}: {shown}"
        );
    }
}
