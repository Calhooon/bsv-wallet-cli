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
}
