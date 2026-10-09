//! Rule 28 (bsv-stack-lean, the ruling of 2026-10-09): every explorer call
//! is either deleted, because a header, a proof or our own index already
//! answers the question, or it is an irreducible break-glass read that says
//! so at its site.
//!
//! This reads the crate's own source. The CLI holds five explorer request
//! lines; each must carry, in the lines directly above it, the reason no
//! header, proof or own-index answer exists. A sixth site, or one that
//! loses its reason, fails here.

use std::path::{Path, PathBuf};

/// `(file, the request line, the tag its reason starts with)`.
const SITES: &[(&str, &str, &str)] = &[
    (
        "src/broadcast_verify.rs",
        r#"format!("{}/tx/hash/{{txid}}", woc_base(chain))"#,
        "Break-glass (Rule 28, C1)",
    ),
    (
        "src/broadcast_verify.rs",
        r#"format!("{}/tx/{{txid}}", bitails_base(chain))"#,
        "Break-glass (Rule 28, C1)",
    ),
    (
        "src/commands/receive.rs",
        r#"format!("{}/tx/{}/beef", base, txid)"#,
        "Break-glass (Rule 28, C2)",
    ),
    (
        "src/commands/sync.rs",
        r#"format!("{}/address/{}/unspent", base, address)"#,
        "Break-glass (Rule 28, C4)",
    ),
    (
        "src/commands/cleanup_abandoned.rs",
        r#"format!("{}/tx/{}/{}/spent", base, src, vout)"#,
        "Break-glass (Rule 28, C7)",
    ),
];

/// How far above a request line its reason may start.
const WINDOW: usize = 12;

/// The files that may hold an HTTP client, each with why.
const HTTP_CLIENTS: &[(&str, &str)] = &[
    (
        "src/broadcast_verify.rs",
        "C1, and the broadcasters' own status reads",
    ),
    ("src/commands/receive.rs", "C2"),
    (
        "src/commands/sync.rs",
        "C4, and the client it hands the spend probe",
    ),
    ("src/commands/cleanup_abandoned.rs", "C7"),
    (
        "src/commands/reconcile_outputs.rs",
        "the client it hands the spend probe (C7)",
    ),
    (
        "src/commands/daemon_tenant.rs",
        "our own relay's /pull, not an explorer",
    ),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A file's code above its test module.
fn code_of(path: &Path) -> String {
    let source = std::fs::read_to_string(path).expect("a source file");
    source
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_string()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root())
        .expect("under the crate")
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn every_explorer_request_line_says_why_at_its_site() {
    for (file, request, tag) in SITES {
        let code = code_of(&root().join(file));
        let lines: Vec<&str> = code.lines().collect();
        let at: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(request))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            at.len(),
            1,
            "{file}: `{request}` appears {} times",
            at.len()
        );
        let from = at[0].saturating_sub(WINDOW);
        assert!(
            lines[from..at[0]].iter().any(|l| l.contains(tag)),
            "{file}:{}: `{request}` has no `{tag}` in the {WINDOW} lines above it",
            at[0] + 1
        );
    }
}

#[test]
fn no_explorer_is_named_outside_the_listed_sites() {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    let listed: Vec<&str> = SITES.iter().map(|(f, _, _)| *f).collect();
    for path in files {
        let file = relative(&path);
        if file == "src/test_support.rs" {
            continue;
        }
        let code = code_of(&path);
        for host in ["api.whatsonchain.com", "api.bitails.io"] {
            assert!(
                !code.contains(host) || listed.contains(&file.as_str()),
                "{file} names the explorer host {host} and is not a listed site"
            );
        }
        let holds_a_client = code.contains("reqwest::Client") || code.contains("use reqwest");
        assert!(
            !holds_a_client || HTTP_CLIENTS.iter().any(|(f, _)| *f == file),
            "{file} holds an HTTP client and is not listed with a reason"
        );
        // A request line of an explorer's shape that the list does not know.
        for (n, line) in code.lines().enumerate() {
            let is_request = line.contains(".get(format!(") || line.contains(".post(format!(");
            if is_request {
                assert!(
                    SITES.iter().any(|(f, r, _)| *f == file && line.contains(r)),
                    "{file}:{}: an unlisted request line: {}",
                    n + 1,
                    line.trim()
                );
            }
        }
    }
}
