use super::tests::{config, refusal, row};
use super::*;

#[test]
fn deterministic_wallets_require_the_complete_hermetic_wall() {
    let seed = "42".repeat(32);
    assert!(config(&[]).wallet.rng_seed.is_none());
    assert_eq!(refusal(&[("E2E_RNG_SEED", &seed)]).variable, "E2E_RNG_SEED");
    let mut wall = vec![
        ("E2E_RNG_SEED", seed.as_str()),
        ("E2E_HERMETIC", "1"),
        ("HTTPS_PROXY", "http://127.0.0.1:3380"),
        ("https_proxy", "http://127.0.0.1:3380"),
        ("HTTP_PROXY", "http://127.0.0.1:3380"),
        ("http_proxy", "http://127.0.0.1:3380"),
        ("ALL_PROXY", "http://127.0.0.1:3380"),
        ("all_proxy", "http://127.0.0.1:3380"),
        ("NO_PROXY", "127.0.0.1,localhost"),
        ("no_proxy", "127.0.0.1,localhost"),
    ];
    let c = config(&wall);
    assert_eq!(c.wallet.rng_seed, Some([0x42; 32]));
    assert_eq!(
        c.wallet.options().rng_seed,
        c.wallet.storage_options().rng_seed
    );
    assert_eq!(row(&c, "E2E_RNG_SEED"), ("<set>".into(), Source::Env));
    for i in 1..wall.len() {
        let removed = wall.remove(i);
        assert_eq!(refusal(&wall).variable, "E2E_RNG_SEED");
        wall.insert(i, removed);
    }
    wall[0].1 = "42";
    assert_eq!(refusal(&wall).variable, "E2E_RNG_SEED");
}
