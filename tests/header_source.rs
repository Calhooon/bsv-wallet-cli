//! P0-1c (bsv-stack-lean #48): the header source that checks every merkle
//! root is the operator's own choice, never a third party's by default.
//! The CLI resolved an unset `CHAINTRACKS_URL` to the public Babbage
//! chaintracks, behind which the toolbox asked WhatsOnChain
//! (`[SRC] bsv-wallet-cli@f21fc37 src/services_env.rs:27-54`). Our own header
//! service, `chaintracks-cloudflare`, documents no public URL (its
//! `wrangler.toml` and README name none), so the setting is required.
//!
//! One test in its own process: it sets the environment.

use bsv_wallet_cli::services_env::services_options_from_env;
use bsv_wallet_toolbox::Chain;

#[test]
fn an_unset_header_source_is_refused_never_a_third_party_default() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let db = dir.path().join("wallet.db");
    let db = db.to_str().unwrap();
    std::env::remove_var("CHAINTRACKS_URL");

    match services_options_from_env(Chain::Main, db) {
        Ok(opts) => panic!(
            "an unset CHAINTRACKS_URL resolved to {:?}",
            opts.chaintracks_url
        ),
        Err(e) => assert!(
            e.to_string().contains("CHAINTRACKS_URL is not set"),
            "the refusal names the setting: {e}"
        ),
    }

    // An explicit header service is used as is, with no explorer behind it.
    std::env::set_var("CHAINTRACKS_URL", "https://headers.example");
    std::env::remove_var("BREAK_GLASS_EXPLORER_HEADERS");
    let opts = services_options_from_env(Chain::Main, db).expect("a header service");
    assert_eq!(
        opts.chaintracks_url.as_deref(),
        Some("https://headers.example")
    );
    assert!(!opts.break_glass_explorer_headers, "off by default");

    // `off` is the explicit choice of none.
    std::env::set_var("CHAINTRACKS_URL", "off");
    let opts = services_options_from_env(Chain::Main, db).expect("off is a choice");
    assert!(opts.chaintracks_url.is_none());

    // Break-glass only when asked for by name.
    std::env::set_var("CHAINTRACKS_URL", "https://headers.example");
    std::env::set_var("BREAK_GLASS_EXPLORER_HEADERS", "1");
    let opts = services_options_from_env(Chain::Main, db).expect("break-glass");
    assert!(opts.break_glass_explorer_headers);
    std::env::remove_var("BREAK_GLASS_EXPLORER_HEADERS");
    std::env::remove_var("CHAINTRACKS_URL");
}
