//! The manifest: the recorded set, so a fixture lost or added by hand is a
//! failure, not a silent pass.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// The manifest: the recorded set, so a fixture lost or added by hand is a
// failure, not a silent pass.
// ---------------------------------------------------------------------------

pub(crate) const MANIFEST: &str = "MANIFEST.json";

#[derive(Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) fixtures: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) notes: std::collections::BTreeMap<String, String>,
}

/// Whether a file's name is a fixture's: a `.json` file.
fn is_fixture_name(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|x| x == "json")
}

/// The fixtures the manifest lists, in its order, after judging the
/// directory against it: every listed file present, every file on disk
/// listed, and nothing but `.json` fixtures beside the manifest.
pub(crate) fn judge_manifest(dir: &Path) -> Result<Vec<String>, Vec<String>> {
    let mut faults = Vec::new();
    let manifest_path = dir.join(MANIFEST);
    let manifest: Manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => return Err(vec![format!("{}: {e}", manifest_path.display())]),
        },
        Err(e) => return Err(vec![format!("{}: {e}", manifest_path.display())]),
    };
    let listed: BTreeSet<&str> = manifest.fixtures.iter().map(String::as_str).collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != MANIFEST)
        .collect();
    for name in &manifest.fixtures {
        if !is_fixture_name(name) {
            faults.push(format!("{name}: listed, not a .json fixture"));
        }
        if !on_disk.contains(name) {
            faults.push(format!("{name}: listed in {MANIFEST}, not on disk"));
        }
    }
    for name in &on_disk {
        if !is_fixture_name(name) {
            faults.push(format!("{name}: on disk, not a .json fixture"));
        } else if !listed.contains(name.as_str()) {
            faults.push(format!("{name}: on disk, not listed in {MANIFEST}"));
        }
    }
    if faults.is_empty() {
        Ok(manifest.fixtures)
    } else {
        Err(faults)
    }
}
