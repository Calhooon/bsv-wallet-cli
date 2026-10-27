//! The broadcaster mode's reading and its rows (#300).

use super::super::tests::{config, refusal, row};
use super::super::{ArcMode, Config, Source};
use bsv_wallet_toolbox::services::ARCADE_V2_MAINNET;

/// `ARC_MODE` (#300): any value but `arcade` or `classic` is refused by
/// name, as is an `ARCADE` that is no switch; the configuration `config`
/// prints holds the default, Arcade, and the refused row as set.
#[test]
fn a_broadcaster_mode_that_is_none_is_refused_by_name() {
    let e = refusal(&[("ARC_MODE", "bogus")]);
    assert_eq!(e.variable, "ARC_MODE");
    assert_eq!(
        e.to_string(),
        "ARC_MODE is \"bogus\", which is not a broadcaster mode: `arcade` (the default) or \
         `classic` (deprecated, the hermetic stub's until #299): set it so, or unset it for \
         the default"
    );
    // ARC_MODE decides even beside a valid ARCADE.
    let e = refusal(&[("ARC_MODE", "arc"), ("ARCADE", "0")]);
    assert_eq!(e.variable, "ARC_MODE");
    let e = refusal(&[("ARCADE", "maybe")]);
    assert_eq!(e.variable, "ARCADE");
    assert!(e.to_string().contains("prefer ARC_MODE"), "{e}");
    let refused = Config::from_pairs(&[("ARC_MODE", "bogus")]).unwrap_err();
    assert_eq!(refused.config.services.arc_mode, ArcMode::Arcade);
    let k = refused
        .config
        .knobs()
        .into_iter()
        .find(|k| k.name == "ARC_MODE")
        .unwrap();
    assert_eq!(k.value.to_string(), "bogus");
    assert_eq!(k.source, Source::Env);
    assert!(k.refused.unwrap().starts_with("a broadcaster mode"));
}

/// The `ARC_MODE` row is the mode in effect (#300), with the source that
/// set it (`ARC_MODE`, or `ARCADE` when it is unset); `ARCADE`'s row is its
/// value as set; `ARC_URL`'s is the broadcaster the mode reaches.
#[test]
fn the_arc_mode_row_is_the_mode_in_effect_and_its_source() {
    let arcade = || "arcade (default)".to_string();
    let c = Config::default();
    assert_eq!(row(&c, "ARC_MODE"), (arcade(), Source::Default));
    assert_eq!(row(&c, "ARCADE"), ("-".to_string(), Source::Default));
    assert_eq!(
        row(&c, "ARC_URL"),
        (ARCADE_V2_MAINNET.to_string(), Source::Default)
    );
    let c = config(&[("ARC_MODE", "Arcade")]);
    assert_eq!(row(&c, "ARC_MODE"), (arcade(), Source::Env));
    let c = config(&[("ARC_MODE", "classic")]);
    assert_eq!(row(&c, "ARC_MODE"), ("classic".to_string(), Source::Env));
    assert_eq!(
        row(&c, "ARC_URL"),
        ("https://arc.taal.com".to_string(), Source::Default)
    );
    let c = config(&[("ARCADE", "0")]);
    assert_eq!(row(&c, "ARC_MODE"), ("classic".to_string(), Source::Env));
    assert_eq!(row(&c, "ARCADE"), ("0".to_string(), Source::Env));
    let c = config(&[("ARCADE", "1")]);
    assert_eq!(row(&c, "ARC_MODE"), (arcade(), Source::Env));
    assert_eq!(row(&c, "ARCADE"), ("1".to_string(), Source::Env));
    // The deprecated TAAL keys stay secrets in either mode.
    for mode in ["arcade", "classic"] {
        let c = config(&[("ARC_MODE", mode), ("TAAL_API_KEY", "key-taal")]);
        assert_eq!(row(&c, "TAAL_API_KEY"), ("<set>".to_string(), Source::Env));
        assert!(!c.log_line().contains("key-taal"));
    }
}
