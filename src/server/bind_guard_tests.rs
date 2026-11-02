//! The bind guard, `refuse_open_bind` (moved from `mod.rs` unchanged).

use super::refuse_open_bind;
use std::net::IpAddr;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn beyond_loopback_needs_a_token_or_the_flag() {
    for open in ["0.0.0.0", "::", "192.168.1.10", "10.0.0.1"] {
        let err = refuse_open_bind(ip(open), None, false).unwrap_err();
        assert!(
            err.to_string().contains("--allow-no-token"),
            "{open}: {err}"
        );
        assert!(
            refuse_open_bind(ip(open), Some(""), false).is_err(),
            "{open}"
        );
        assert!(
            refuse_open_bind(ip(open), Some("t"), false).is_ok(),
            "{open}"
        );
        assert!(refuse_open_bind(ip(open), None, true).is_ok(), "{open}");
    }
}

#[test]
fn loopback_is_unchanged() {
    for lo in ["127.0.0.1", "127.0.0.2", "::1", "::ffff:127.0.0.1"] {
        assert!(refuse_open_bind(ip(lo), None, false).is_ok(), "{lo}");
    }
}
