//! The source scan the `tests/arch_*.rs` rules share (#73): the crate's own
//! `src/` read as text, its comments and its test code blanked, so a rule
//! counts only what the shipped binary holds.
//!
//! Two views of each file, both keeping every newline so a hit keeps its
//! line number:
//! - `code`: comments and test code blanked, string literals kept (the SQL
//!   rule, the prefix rule and the explorer rule read string contents).
//! - `bare`: `code` with string and char literal contents blanked too (the
//!   unwrap, spawn and env rules count calls, never a word in a message).
//!
//! Test code is every item a `#[cfg(test)]` attribute carries (a
//! `mod tests { .. }` block, a single function, a statement) and every file
//! such an attribute declares (`#[cfg(test)] mod tests;`, with its
//! `#[path = ".."]` when it has one): `src/test_support.rs`,
//! `src/tracker_words_witnesses.rs`, the `tests.rs` files.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The crate under test: cargo's `CARGO_MANIFEST_DIR` at run time (a test
/// binary reused from a shared target dir scans the checkout it runs in),
/// else the one it was built from.
pub fn root() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// One source file of `src/`, in its two views.
pub struct Source {
    /// The path from the crate root, `/`-separated.
    pub file: String,
    pub code: String,
    pub bare: String,
}

/// Every `.rs` file under `src/` that is not test code, sorted.
pub fn sources() -> Vec<Source> {
    let mut paths = Vec::new();
    rust_files(&root().join("src"), &mut paths);
    paths.sort();
    let tests = test_files(&paths);
    paths
        .into_iter()
        .filter(|p| !tests.contains(p))
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("a source file");
            let (code, bare) = views(&text);
            Source {
                file: relative(&p),
                code,
                bare,
            }
        })
        .collect()
}

/// The files a `#[cfg(test)] mod name;` declares, resolved as rustc does:
/// beside `lib.rs`, `main.rs` and `mod.rs`, in a directory of the declaring
/// file's name otherwise, at its `#[path]` when one is given.
pub fn test_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(p).expect("a source file");
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let dir = p.parent().expect("a directory");
        let stem = p.file_stem().expect("a name").to_string_lossy().to_string();
        let home = if ["lib", "main", "mod"].contains(&stem.as_str()) {
            dir.to_path_buf()
        } else {
            dir.join(&stem)
        };
        for (i, line) in lines.iter().enumerate() {
            if *line != "#[cfg(test)]" {
                continue;
            }
            let mut path = None;
            for next in &lines[i + 1..] {
                if let Some(rest) = next.strip_prefix("#[path = \"") {
                    path = rest.strip_suffix("\"]").map(|r| dir.join(r));
                    continue;
                }
                if next.starts_with("#[") {
                    continue;
                }
                let decl = next
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub ");
                if let Some(name) = decl.strip_prefix("mod ").and_then(|d| d.strip_suffix(';')) {
                    let file = path.take().unwrap_or_else(|| {
                        let flat = home.join(format!("{name}.rs"));
                        if flat.exists() {
                            flat
                        } else {
                            home.join(name).join("mod.rs")
                        }
                    });
                    out.push(file);
                }
                break;
            }
        }
    }
    out
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

/// The two views of one file's text: `(code, bare)`.
pub fn views(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut code = chars.clone();
    let mut bare = chars.clone();
    let blank = |v: &mut Vec<char>, from: usize, to: usize| {
        for c in &mut v[from..to] {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            let end = (i..n).find(|&j| chars[j] == '\n').unwrap_or(n);
            blank(&mut code, i, end);
            blank(&mut bare, i, end);
            i = end;
        } else if c == '/' && next == Some('*') {
            let (mut depth, mut j) = (1, i + 2);
            while j < n && depth > 0 {
                if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut code, i, j);
            blank(&mut bare, i, j);
            i = j;
        } else if c == 'r'
            && (i == 0
                || !ident(chars[i - 1])
                || (chars[i - 1] == 'b' && (i < 2 || !ident(chars[i - 2]))))
            && matches!(next, Some('"') | Some('#'))
        {
            // A raw string `r"..."`, `r#"..."#`, or a raw identifier `r#name`.
            let hashes = (i + 1..n).take_while(|&j| chars[j] == '#').count();
            let open = i + 1 + hashes;
            if chars.get(open) != Some(&'"') {
                i += 1;
                continue;
            }
            let mut j = open + 1;
            let end = loop {
                if j >= n {
                    break n;
                }
                if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                    break j;
                }
                j += 1;
            };
            blank(&mut bare, open + 1, end);
            i = (end + 1 + hashes).min(n);
        } else if c == '"' {
            let mut j = i + 1;
            while j < n && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let end = j.min(n);
            blank(&mut bare, i + 1, end);
            i = end + 1;
        } else if c == '\'' {
            // A char literal `'x'`, `'\n'`, `'\u{..}'`, or a lifetime `'a`.
            if next == Some('\\') {
                let end = (i + 2..n).find(|&j| chars[j] == '\'').unwrap_or(n);
                blank(&mut bare, i + 1, end);
                i = end + 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                blank(&mut bare, i + 1, i + 2);
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    // Test items: each `#[cfg(test)]` through the end of the item it carries.
    let attr: Vec<char> = "#[cfg(test)]".chars().collect();
    let mut i = 0;
    while i + attr.len() <= n {
        if bare[i..i + attr.len()] != attr[..] {
            i += 1;
            continue;
        }
        let mut j = i + attr.len();
        let mut depth = 0usize;
        let end = loop {
            if j >= n {
                break n;
            }
            match bare[j] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break j + 1;
                    }
                }
                ';' if depth == 0 => break j + 1,
                _ => {}
            }
            j += 1;
        };
        blank(&mut code, i, end);
        blank(&mut bare, i, end);
        i = end;
    }
    (code.into_iter().collect(), bare.into_iter().collect())
}

/// One hit: the file, its 1-based line, the line's text.
pub struct Hit {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// One line of a source in its two views, with its 1-based number.
pub struct Line<'a> {
    pub source: &'a Source,
    pub number: usize,
    pub code: &'a str,
    pub bare: &'a str,
}

impl Line<'_> {
    /// The text of the line's string literals (the code view where the bare
    /// view is blank), each run of it separated by a space.
    pub fn strings(&self) -> String {
        self.code
            .chars()
            .zip(self.bare.chars())
            .map(|(c, b)| if c != b { c } else { ' ' })
            .collect()
    }
}

/// One hit per site `sites` counts on each line of every source.
pub fn hits(sites: impl Fn(&Line) -> usize) -> Vec<Hit> {
    let mut out = Vec::new();
    for s in sources() {
        let raw = std::fs::read_to_string(root().join(&s.file)).expect("a source file");
        let raw: Vec<&str> = raw.lines().collect();
        for (n, (code, bare)) in s.code.lines().zip(s.bare.lines()).enumerate() {
            let line = Line {
                source: &s,
                number: n + 1,
                code,
                bare,
            };
            for _ in 0..sites(&line) {
                out.push(Hit {
                    file: s.file.clone(),
                    line: n + 1,
                    text: raw.get(n).unwrap_or(&"").trim().to_string(),
                });
            }
        }
    }
    out
}

/// Counts `hits` per file against `allowed` (file, ceiling, why), and fails
/// with every offending file and line: a file not listed, a file over its
/// ceiling, and a file under it (the allowlist is lowered to the new count
/// in the same PR, so it can only shrink).
pub fn judge(rule: &str, found: &[Hit], allowed: &[(&str, usize, &str)]) {
    let mut per_file: BTreeMap<&str, Vec<&Hit>> = BTreeMap::new();
    for h in found {
        per_file.entry(h.file.as_str()).or_default().push(h);
    }
    let mut faults = Vec::new();
    for (file, hs) in &per_file {
        let ceiling = allowed
            .iter()
            .find(|(f, _, _)| f == file)
            .map(|(_, c, _)| *c);
        match ceiling {
            Some(c) if hs.len() == c => {}
            Some(c) if hs.len() < c => faults.push(format!(
                "{file}: {} sites, the allowlist says {c}; lower it to {} (it only shrinks)",
                hs.len(),
                hs.len()
            )),
            _ => {
                let c = ceiling.unwrap_or(0);
                faults.push(format!(
                    "{file}: {} sites, the allowlist allows {c}:",
                    hs.len()
                ));
                for h in hs {
                    faults.push(format!("  {}:{}: {}", h.file, h.line, h.text));
                }
            }
        }
    }
    for (file, c, _) in allowed {
        if !per_file.contains_key(file) {
            faults.push(format!(
                "{file}: no sites, the allowlist says {c}; remove its entry"
            ));
        }
    }
    assert!(
        faults.is_empty(),
        "{rule} (docs/ARCH-RULES.md):\n{}",
        faults.join("\n")
    );
}

/// The 1-based lines `[from, to]` of the item whose header holds `needle`
/// (in `bare`), through its closing brace; `None` if the file has none.
pub fn item_lines(s: &Source, needle: &str) -> Option<(usize, usize)> {
    let at = s.bare.find(needle)?;
    let open = at + s.bare[at..].find('{')?;
    let mut depth = 0usize;
    let mut close = None;
    for (k, c) in s.bare[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + k);
                    break;
                }
            }
            _ => {}
        }
    }
    let line_of = |byte: usize| s.bare[..byte].matches('\n').count() + 1;
    Some((line_of(at), line_of(close?)))
}

/// How many times `needle` occurs in `line`.
pub fn count(line: &str, needle: &str) -> usize {
    line.matches(needle).count()
}
