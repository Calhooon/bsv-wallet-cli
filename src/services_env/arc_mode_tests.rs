//! The broadcaster mode's tests (#300): Arcade the default, classic the
//! one opt-out, `GorillaPool`'s ARC never a rung, the TAAL keys classic
//! only, a testnet wallet without `ARC_URL` refused; every one through the
//! configuration, as every command builds its services.

use super::*;
use crate::broadcast_verify::post_beef_ladder;
use crate::config::{Config, Flags, Refused};
use bsv_wallet_toolbox::{
    PROVIDER_ARCADE_V2, PROVIDER_BITAILS, PROVIDER_GORILLAPOOL_ARC, PROVIDER_TAAL_ARC,
    PROVIDER_WHATSONCHAIN,
};

/// The configuration of `pairs` on `chain`, with `CHAINTRACKS_URL=off`, as
/// `main` reads it: a refusal holds the configuration `config` prints.
fn read(chain: Chain, pairs: &[(&str, &str)]) -> Result<Config, Refused> {
    let flags = Flags {
        testnet: chain == Chain::Test,
        ..Flags::default()
    };
    let vars = pairs
        .iter()
        .chain(&[("CHAINTRACKS_URL", "off")])
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()));
    Config::from_vars(&flags, vars)
}

/// [`read`], a refusal an error.
fn configured(chain: Chain, pairs: &[(&str, &str)]) -> Result<Config> {
    read(chain, pairs).map_err(|refused| anyhow::anyhow!(refused))
}

/// The toolbox's options from `pairs` on `chain`, through the
/// configuration, as every command builds them.
fn built(chain: Chain, pairs: &[(&str, &str)]) -> Result<ServicesOptions> {
    let config = configured(chain, pairs)?;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("w.db");
    services_options(&config.services, chain, db.to_str().unwrap())
}

#[test]
fn arcade_is_the_default_mode() {
    let opts = built(Chain::Main, &[]).unwrap();
    assert!(opts.arcade_v2);
    assert_eq!(opts.arc_url, ARCADE_V2_MAINNET);
    assert_eq!(
        configured(Chain::Main, &[]).unwrap().services.arc_mode,
        ArcMode::Arcade
    );
}

#[test]
fn classic_is_the_one_opt_out() {
    for pairs in [
        [("ARC_MODE", "classic")],
        [("ARC_MODE", " CLASSIC ")],
        [("ARCADE", "0")],
        [("ARCADE", "false")],
        [("ARCADE", "No")],
    ] {
        let opts = built(Chain::Main, &pairs).unwrap();
        assert!(!opts.arcade_v2, "{pairs:?}");
        assert_eq!(
            opts.arc_url,
            ServicesOptions::mainnet().arc_url,
            "{pairs:?}"
        );
    }
    // ARC_MODE wins over ARCADE when both are set.
    let opts = built(Chain::Main, &[("ARC_MODE", "classic"), ("ARCADE", "1")]).unwrap();
    assert!(!opts.arcade_v2);
}

#[test]
fn arcade_named_in_any_case_is_arcade() {
    for pairs in [
        &[("ARC_MODE", "arcade")][..],
        &[("ARC_MODE", "ARCADE")][..],
        &[("ARC_MODE", " Arcade "), ("ARCADE", "0")][..],
        &[("ARC_MODE", " "), ("ARCADE", "1")][..],
        &[("ARCADE", "1")][..],
        &[("ARCADE", "TRUE")][..],
        &[("ARCADE", "yes")][..],
    ] {
        assert!(built(Chain::Main, pairs).unwrap().arcade_v2, "{pairs:?}");
    }
}

#[test]
fn an_unknown_mode_is_refused_by_name() {
    let err = built(Chain::Main, &[("ARC_MODE", "bogus")])
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("ARC_MODE is \"bogus\""), "{err}");
    assert!(
        err.contains("`arcade` (the default)") && err.contains("`classic`"),
        "{err}"
    );
    let err = built(Chain::Main, &[("ARCADE", "maybe")])
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("ARCADE is \"maybe\""), "{err}");
}

#[test]
fn gorillapool_arc_is_never_a_rung() {
    assert!(ServicesOptions::mainnet().arc_gorillapool_url.is_some());
    for chain in [Chain::Main, Chain::Test] {
        // A testnet wallet in Arcade mode names its endpoint.
        let arcade: &[(&str, &str)] = match chain {
            Chain::Main => &[],
            Chain::Test => &[("ARC_URL", "https://arcade-test.invalid")],
        };
        for pairs in [
            arcade,
            &[("ARC_MODE", "classic")][..],
            &[
                ("ARC_MODE", "classic"),
                ("ARC_GORILLAPOOL_URL", "https://gp.example"),
            ][..],
        ] {
            let opts = built(chain, pairs).unwrap();
            assert_eq!(opts.arc_gorillapool_url, None, "{chain:?} {pairs:?}");
        }
    }
    // BananaBlocks, its explorer, keeps its rung.
    let opts = built(Chain::Main, &[]).unwrap();
    assert_eq!(
        opts.bananablocks_base_for(Chain::Main).as_deref(),
        Some("https://bananablocks.com/api/v1")
    );
}

/// `ARC_GORILLAPOOL_URL` (#74, #172, #300): the broadcast verifier's
/// third-party store, the base its knob shows; never the toolbox's
/// broadcaster. Unset is the chain's default, empty or blank leaves it
/// out, a URL moves it, a refused value is the default.
#[test]
fn the_gorillapool_knob_is_the_verifiers_store_alone() {
    let shown = |pairs: &[(&str, &str)], chain| {
        read(chain, pairs)
            .unwrap_or_else(|refused| *refused.config)
            .services
            .explorers
            .bases
            .gorillapool
    };
    assert_eq!(
        shown(&[], Chain::Main),
        ServicesOptions::mainnet().arc_gorillapool_url
    );
    assert_eq!(shown(&[], Chain::Test), None);
    for blank in ["", "  "] {
        assert_eq!(shown(&[("ARC_GORILLAPOOL_URL", blank)], Chain::Main), None);
    }
    assert_eq!(
        shown(
            &[("ARC_GORILLAPOOL_URL", " http://127.0.0.1:3380/arc/ ")],
            Chain::Main
        )
        .as_deref(),
        Some("http://127.0.0.1:3380/arc")
    );
    assert_eq!(
        shown(&[("ARC_GORILLAPOOL_URL", "nope")], Chain::Main),
        ServicesOptions::mainnet().arc_gorillapool_url
    );
}

#[test]
fn arcade_mode_passes_no_classic_arc_config() {
    let keys = [("TAAL_API_KEY", "k1"), ("MAIN_TAAL_API_KEY", "k2")];
    assert!(built(Chain::Main, &keys).unwrap().arc_config.is_none());
    let classic = built(
        Chain::Main,
        &[("ARC_MODE", "classic"), ("TAAL_API_KEY", "k1")],
    )
    .unwrap();
    assert_eq!(
        classic.arc_config.and_then(|c| c.api_key).as_deref(),
        Some("k1")
    );
    let classic = built(
        Chain::Main,
        &[("ARC_MODE", "classic"), ("MAIN_TAAL_API_KEY", "k2")],
    )
    .unwrap();
    assert_eq!(
        classic
            .arc_config
            .and_then(|c| c.headers)
            .and_then(|h| h.get("Authorization").cloned())
            .as_deref(),
        Some("k2")
    );
}

#[test]
fn arc_url_moves_the_broadcaster_in_either_mode() {
    let arcade = built(Chain::Main, &[("ARC_URL", "https://arcade.invalid")]).unwrap();
    assert!(arcade.arcade_v2);
    assert_eq!(arcade.arc_url, "https://arcade.invalid");
    let classic = built(
        Chain::Main,
        &[("ARC_MODE", "classic"), ("ARC_URL", "https://arc.invalid")],
    )
    .unwrap();
    assert!(!classic.arcade_v2);
    assert_eq!(classic.arc_url, "https://arc.invalid");
}

#[test]
fn a_blank_arc_url_is_the_arcade_default() {
    for blank in ["", "  "] {
        let opts = built(Chain::Main, &[("ARC_URL", blank)]).unwrap();
        assert!(opts.arcade_v2, "{blank:?}");
        assert_eq!(opts.arc_url, ARCADE_V2_MAINNET, "{blank:?}");
        assert_eq!(post_beef_ladder(&opts)[0], PROVIDER_ARCADE_V2, "{blank:?}");
    }
    assert_eq!(
        arcade_url_for(Chain::Main, Some(" ")).unwrap(),
        ARCADE_V2_MAINNET
    );
}

#[test]
fn testnet_arcade_without_arc_url_is_refused_by_name() {
    for pairs in [
        &[][..],
        &[("ARC_URL", " ")][..],
        &[("ARC_MODE", "arcade")][..],
    ] {
        let err = built(Chain::Test, pairs).unwrap_err().to_string();
        assert_eq!(err, NO_TESTNET_ARCADE_DEFAULT, "{pairs:?}");
    }
    assert_eq!(
        arcade_url_for(Chain::Test, None).unwrap_err().to_string(),
        "no testnet Arcade default; set ARC_URL (or ARC_MODE=classic, deprecated)"
    );
    // ARC_URL set: it runs, on that endpoint, Arcade first.
    let opts = built(Chain::Test, &[("ARC_URL", "https://arcade-test.invalid")]).unwrap();
    assert!(opts.arcade_v2);
    assert_eq!(opts.arc_url, "https://arcade-test.invalid");
    // The deprecated classic knob is the chain's TAAL testnet ARC.
    let opts = built(Chain::Test, &[("ARC_MODE", "classic")]).unwrap();
    assert!(!opts.arcade_v2);
    assert_eq!(opts.arc_url, "https://arc-test.taal.com");
}

#[test]
fn the_taal_warning_names_the_provider_that_still_posts() {
    assert!(TAAL_KEY_UNUSED_WARNING.contains("arc is just deprecated, arcade is the standard"));
    assert!(TAAL_KEY_UNUSED_WARNING.contains("toolbox #317"));
    assert!(!TAAL_KEY_UNUSED_WARNING.contains("no classic ARC rung runs"));
}

#[test]
fn the_post_ladder_reads_arcade_first() {
    assert_eq!(
        post_beef_ladder(&built(Chain::Main, &[]).unwrap()),
        [
            PROVIDER_ARCADE_V2,
            PROVIDER_TAAL_ARC,
            PROVIDER_BITAILS,
            PROVIDER_WHATSONCHAIN
        ]
    );
    assert_eq!(
        post_beef_ladder(&built(Chain::Main, &[("ARC_MODE", "classic")]).unwrap()),
        [PROVIDER_TAAL_ARC, PROVIDER_BITAILS, PROVIDER_WHATSONCHAIN]
    );
    // The toolbox's collection holds as many providers as the ladder
    // names: GorillaPool's presence alone would make them differ.
    for pairs in [&[][..], &[("ARC_MODE", "classic")][..]] {
        let opts = built(Chain::Main, pairs).unwrap();
        let ladder = post_beef_ladder(&opts);
        let services = bsv_wallet_toolbox::Services::with_options(Chain::Main, opts).unwrap();
        assert_eq!(
            services.post_beef_count().unwrap(),
            ladder.len(),
            "{pairs:?}"
        );
    }
    // The toolbox's own default, for the rule the ladder mirrors.
    assert_eq!(
        post_beef_ladder(&ServicesOptions::mainnet()),
        [
            PROVIDER_TAAL_ARC,
            PROVIDER_GORILLAPOOL_ARC,
            PROVIDER_BITAILS,
            PROVIDER_WHATSONCHAIN
        ]
    );
}
