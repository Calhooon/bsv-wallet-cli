//! The fuzz targets' properties on stable (#396): every file of each
//! target's committed seed corpus (`fuzz/corpus/<target>/`) through the
//! function its `fuzz_target!` calls (`fuzz/src/props/`), so the
//! assertions the five targets carry over the floored files judge the
//! Nightly's mutants as any other test target does (docs/NIGHTLY.md).
//!
//! The fuzz crate's library is compiled here by `#[path]`: it names no
//! `libfuzzer-sys`, and its other dependencies are the package's own or
//! its dev-dependencies. A missing or empty corpus fails by the target's
//! name, a failing seed is named before its panic goes on, and each test
//! prints its count (`cargo test --test fuzz_replay -- --nocapture`).

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use arbitrary::{Arbitrary, Unstructured};

#[allow(dead_code)]
#[path = "../fuzz/src/lib.rs"]
mod fuzz;

use fuzz::props;

/// `beef_reader`'s property compiles `src/atomic_beef.rs`, whose unit tests
/// build under this target's `cfg(test)` and name the tree's builders as
/// `crate::test_support`: the same file (`src/test_support/tx.rs`) the fuzz
/// library mounts. Its three tests run here too, beside the binary's copy.
mod test_support {
    pub use super::fuzz::{p2pkh, raw_tx, txid_of};
}

/// The five targets, as `fuzz/fuzz_targets/` names them.
const TARGETS: [&str; 5] = [
    "allow_pending",
    "beef_reader",
    "brc29_remittance",
    "explorer_bodies",
    "served_json",
];

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz")
}

/// The target's committed seeds, by file name.
fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = fuzz_dir().join("corpus").join(target);
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!("{target}: no seed corpus at {}: {e}", dir.display());
    });
    let mut seeds: Vec<(String, Vec<u8>)> = entries
        .map(|entry| entry.expect("a corpus entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            let bytes = fs::read(&path).expect("a seed's bytes");
            (name, bytes)
        })
        .collect();
    assert!(
        !seeds.is_empty(),
        "{target}: the seed corpus at {} is empty",
        dir.display()
    );
    seeds.sort();
    seeds
}

/// Every seed of `target` through `check`, the failing seed named.
fn replay(target: &str, check: impl Fn(&[u8])) {
    let seeds = seeds(target);
    for (name, bytes) in &seeds {
        if let Err(failure) = panic::catch_unwind(AssertUnwindSafe(|| check(bytes))) {
            eprintln!("{target}: the seed {name} failed the property");
            panic::resume_unwind(failure);
        }
    }
    println!("fuzz_replay {target}: {} seeds replayed", seeds.len());
}

#[test]
fn every_fuzz_target_is_replayed() {
    let mut names: Vec<String> = fs::read_dir(fuzz_dir().join("fuzz_targets"))
        .expect("fuzz/fuzz_targets")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| {
            let stem = path.file_stem().expect("a stem");
            stem.to_string_lossy().into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names, TARGETS,
        "a fuzz target without its replay here, or a replay without its target"
    );
}

#[test]
fn allow_pending() {
    replay("allow_pending", props::allow_pending::check);
}

#[test]
fn beef_reader() {
    replay("beef_reader", props::beef_reader::check);
}

/// The bytes decoded as libfuzzer-sys 0.4's `fuzz_target!` decodes a
/// structured input: too few bytes for the type's size hint, or an
/// `arbitrary` error, and the binary runs nothing on them, so a seed the
/// binary would reject fails here rather than judging nothing.
#[test]
fn brc29_remittance() {
    use props::brc29_remittance::{check, Input};
    replay("brc29_remittance", |bytes| {
        assert!(
            bytes.len() >= Input::size_hint(0).0,
            "shorter than the input's size hint"
        );
        let input = Input::arbitrary_take_rest(Unstructured::new(bytes))
            .expect("the bytes decode as the target's input");
        check(input);
    });
}

#[test]
fn explorer_bodies() {
    replay("explorer_bodies", props::explorer_bodies::check);
}

#[test]
fn served_json() {
    replay("served_json", props::served_json::check);
}
