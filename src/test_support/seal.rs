//! The network seal of a test process (#477): every non-loopback request
//! to a closed local port. Mounted by `src/test_support.rs` and by a test
//! target outside the crate that builds a concrete `Services`
//! (`tests/served_internalize_door.rs`).

/// A closed local port: the proxy of every request [`seal_network`] keeps
/// off the network.
pub const CLOSED: &str = "http://127.0.0.1:9";

/// Send every non-loopback request of this process to [`CLOSED`], once,
/// before a client is built: the post ladder's Bitails and WhatsOnChain
/// rungs have no base to point at a mock, and through them the handler
/// mutant witnesses posted to mainnet and waited on it, 4 to 65 seconds a
/// test (#477). A request there is a fault ("could not look"), at once;
/// loopback mocks are unproxied. The wire harness seals its worlds the
/// same way (`tests/wire_support`, `quiet_environment`).
pub fn seal_network() {
    static SEALED: std::sync::Once = std::sync::Once::new();
    SEALED.call_once(|| {
        for var in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            std::env::set_var(var, CLOSED);
        }
        for var in ["NO_PROXY", "no_proxy"] {
            std::env::set_var(var, "127.0.0.1,localhost");
        }
    });
}
