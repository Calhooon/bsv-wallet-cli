//! The served pause before the spend gate's header re-read (#506): every
//! place that builds it serves [`HEADER_RETRY_PAUSE`], and the router hands
//! the server's on to the gate. The gate's own tests run a short pause they
//! supply, so this is the one check of the served value.

use super::{gate_options, ServerConfig};
use crate::config::Config;
use crate::spend_gate::{GateOptions, HEADER_RETRY_PAUSE};
use bsv_wallet_toolbox::Chain;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn every_built_gate_serves_the_two_second_pause() {
    let settings = Arc::new(Config::default());
    assert_eq!(
        HEADER_RETRY_PAUSE,
        Duration::from_secs(2),
        "the served pause is two seconds"
    );
    assert_eq!(
        GateOptions::default().header_retry_pause,
        HEADER_RETRY_PAUSE,
        "GateOptions::default serves the pause"
    );
    assert_eq!(
        GateOptions::from_config(&settings).header_retry_pause,
        HEADER_RETRY_PAUSE,
        "GateOptions::from_config serves the pause"
    );
    assert_eq!(
        ServerConfig::default().header_retry_pause,
        HEADER_RETRY_PAUSE,
        "ServerConfig::default serves the pause"
    );
    let built = ServerConfig::from_config(&settings, Chain::Main, None).unwrap();
    assert_eq!(
        built.header_retry_pause, HEADER_RETRY_PAUSE,
        "ServerConfig::from_config serves the pause"
    );
    assert_eq!(
        gate_options(&built).header_retry_pause,
        HEADER_RETRY_PAUSE,
        "the router's gate serves the server's pause"
    );
}

#[test]
fn the_router_hands_the_servers_pause_to_the_gate() {
    let config = ServerConfig {
        header_retry_pause: Duration::from_millis(7),
        ..ServerConfig::default()
    };
    assert_eq!(
        gate_options(&config).header_retry_pause,
        Duration::from_millis(7),
        "the router's gate reads ServerConfig's pause"
    );
}
