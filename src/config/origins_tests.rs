//! `ALLOWED_ORIGINS` read through the typed config (#411): the browser
//! gate's exact origins, or the list refused by name.

use super::tests::{config, refusal, row};
use super::*;

/// #411: `ALLOWED_ORIGINS` holds exact origins as a browser sends them;
/// unset or blank admits none, and an entry in any other spelling refuses
/// the list by name at start, never admitting more or less than written.
#[test]
fn allowed_origins_are_exact_origins_or_refused() {
    assert!(Config::default().server.allowed_origins.is_empty());
    assert!(config(&[("ALLOWED_ORIGINS", " ")])
        .server
        .allowed_origins
        .is_empty());
    let c = config(&[(
        "ALLOWED_ORIGINS",
        "http://localhost, https://app.example:8443,,http://127.0.0.1:3000",
    )]);
    assert_eq!(
        c.server.allowed_origins,
        [
            "http://localhost",
            "https://app.example:8443",
            "http://127.0.0.1:3000"
        ]
    );
    assert_eq!(
        row(&c, "ALLOWED_ORIGINS").0,
        "http://localhost,https://app.example:8443,http://127.0.0.1:3000"
    );
    assert_eq!(row(&Config::default(), "ALLOWED_ORIGINS").0, "none");
    for entry in [
        "*",
        "localhost",
        "localhost:3000",
        "http://localhost/",
        "http://localhost/app",
        "http://localhost:80",
        "HTTP://LOCALHOST",
        "http://user@localhost",
        "http://localhost?x=1",
        "ftp://localhost",
        "null",
        "http://*.example",
    ] {
        let e = refusal(&[("ALLOWED_ORIGINS", &format!("http://localhost,{entry}"))]);
        assert_eq!(e.variable, "ALLOWED_ORIGINS", "{entry}");
        assert!(e.to_string().contains("exact origins"), "{entry}: {e}");
    }
}
