//! The source scan the `tests/arch_*.rs` rules share (#73): the crate's own
//! `src/` read as text, its comments and its test code blanked, so a rule
//! counts only what the shipped binary holds.
//!
//! Two views of each file, both keeping every newline so a hit keeps its
//! line number:
//! - `code`: comments and test code blanked, string literals kept (the SQL
//!   rule and the explorer rule read string contents).
//! - `bare`: `code` with string and char literal contents blanked too (the
//!   unwrap, spawn, env and prefix rules count calls, never a word in a
//!   message).
//!
//! Test code is every item a `#[cfg(test)]` attribute carries (a
//! `mod tests { .. }` block, a function, a statement, a field, a variant, a
//! match arm) and every file that is test code whole, as `tools/ratchet`'s
//! `test_files` reads it: a file under the crate's top-level `tests/`,
//! `benches/` or `fuzz/`; a file a `#[cfg(test)] mod x;` declares (`x.rs` or
//! `x/mod.rs`, or its `#[path = ".."]`); a file a plain `mod y;` declares
//! inside a file that is already test code; and every file under such a
//! module's directory; iterated until nothing grows. A file's name decides
//! nothing. All of it is read from the comment-blanked text, so an
//! attribute spelled in a comment or a string literal carries nothing and
//! declares nothing.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The crate under test: cargo's `CARGO_MANIFEST_DIR` at run time (a test
/// binary reused from a shared target dir scans the checkout it runs in),
/// else the one it was built from.
pub fn root() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from)
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
    let paths = rust_files(&root().join("src"));
    let tests = test_files(&paths);
    paths
        .into_iter()
        .filter(|p| !tests.contains(p))
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("a source file");
            let (code, bare) = views(&text);
            let file = p.strip_prefix(root()).expect("under the crate");
            Source {
                file: file.to_string_lossy().replace('\\', "/"),
                code,
                bare,
            }
        })
        .collect()
}

/// The attribute that makes an item, or the file it declares, test code.
const CFG_TEST: &str = "#[cfg(test)]";

/// The files that are test code whole, as `tools/ratchet`'s `test_files`
/// reads them: a file under the crate's top-level `tests/`, `benches/` or
/// `fuzz/`; a file a `#[cfg(test)] mod x;` declares (`x.rs` or `x/mod.rs`,
/// or its `#[path]`); a file a plain `mod y;` declares inside a file that
/// is already test code; and every file of `paths` under such a module's
/// directory. Iterated until nothing grows. A file's name decides nothing.
pub fn test_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let root = root();
    let mut lexed_files = BTreeMap::new();
    for p in paths {
        let text = std::fs::read_to_string(p).expect("a source file");
        lexed_files.insert(p.clone(), lexed(&text));
    }
    let mut test: Vec<PathBuf> = paths
        .iter()
        .filter(|p| {
            p.strip_prefix(&root)
                .ok()
                .and_then(|r| r.iter().next())
                .is_some_and(|top| ["tests", "benches", "fuzz"].iter().any(|t| top == *t))
        })
        .cloned()
        .collect();
    for (p, (code, bare)) in &lexed_files {
        test.extend(declared_test_files(p, code, bare));
    }
    loop {
        let mut grown = Vec::new();
        for t in &test {
            if let Some((code, bare)) = lexed_files.get(t) {
                grown.extend(declared_files(t, code, bare));
            }
            let dir = if t.file_name().is_some_and(|n| n == "mod.rs") {
                t.parent().expect("a directory").to_path_buf()
            } else {
                t.with_extension("")
            };
            grown.extend(paths.iter().filter(|p| p.starts_with(&dir)).cloned());
        }
        grown.retain(|g| !test.contains(g));
        grown.sort();
        grown.dedup();
        if grown.is_empty() {
            return test;
        }
        test.extend(grown);
    }
}

/// The directory a file's `mod name;` declarations resolve in: beside
/// `lib.rs`, `main.rs` and `mod.rs`, in a directory of the file's name
/// otherwise.
fn module_home(file: &Path) -> PathBuf {
    let dir = file.parent().expect("a directory");
    let stem = file
        .file_stem()
        .expect("a name")
        .to_string_lossy()
        .to_string();
    if ["lib", "main", "mod"].contains(&stem.as_str()) {
        dir.to_path_buf()
    } else {
        dir.join(&stem)
    }
}

/// The files one file's `#[cfg(test)] mod name;` declarations resolve to,
/// as rustc resolves them (`module_home`, then `name.rs` or
/// `name/mod.rs`), at its `#[path]` when one is given. Read from the
/// file's comment-blanked views (`lexed`): `bare` says where an attribute
/// and a declaration are, `code` holds the `#[path = ".."]` text. A
/// `mod tests { .. }` block declares no file.
pub fn declared_test_files(file: &Path, code: &[char], bare: &[char]) -> Vec<PathBuf> {
    let starts = occurrences(bare, CFG_TEST)
        .into_iter()
        .map(|at| at + CFG_TEST.len());
    attributed_mods(file, code, bare, starts)
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// The files every `mod name;` of one file resolves to, whatever its
/// attributes, at its `#[path]` when one is given: the modules a file that
/// is test code declares are test code too.
pub fn declared_files(file: &Path, code: &[char], bare: &[char]) -> Vec<PathBuf> {
    let pathed = attributed_mods(file, code, bare, occurrences(bare, "#[path = \""));
    let home = module_home(file);
    let mut out: Vec<PathBuf> = Vec::new();
    for at in occurrences(bare, "mod") {
        let boundary = at == 0 || !is_ident(bare[at - 1]);
        if !boundary || pathed.iter().any(|(k, _)| *k == at) {
            continue;
        }
        if let Some(name) = declared_mod(bare, at) {
            out.push(module_file(&home, &name));
        }
    }
    out.extend(pathed.into_iter().map(|(_, path)| path));
    out
}

/// The `mod name;` declarations whose attributes are read from each of
/// `from`: where the `mod` keyword stands and the file it resolves to, at
/// a `#[path]` among the attributes when one is given.
fn attributed_mods(
    file: &Path,
    code: &[char],
    bare: &[char],
    from: impl IntoIterator<Item = usize>,
) -> Vec<(usize, PathBuf)> {
    let dir = file.parent().expect("a directory");
    let home = module_home(file);
    let mut out = Vec::new();
    for at in from {
        let (j, path) = further_attributes(dir, code, bare, at);
        if let Some(name) = declared_mod(bare, j) {
            let path = path.unwrap_or_else(|| module_file(&home, &name));
            out.push((past_visibility(bare, j), path));
        }
    }
    out
}

/// Walks the further attributes from `from`: where the item they carry
/// starts, and the file a `#[path = ".."]` among them names.
fn further_attributes(
    dir: &Path,
    code: &[char],
    bare: &[char],
    from: usize,
) -> (usize, Option<PathBuf>) {
    let n = bare.len();
    let mut j = from;
    let mut path = None;
    loop {
        j = skip_space(bare, j);
        if !starts(bare, j, "#[") {
            return (j, path);
        }
        let close = (j..n).find(|&k| bare[k] == ']').unwrap_or(n);
        if starts(bare, j, "#[path = \"") {
            let open = j + "#[path = \"".len();
            let end = (open..close)
                .rev()
                .find(|&k| code[k] == '"')
                .unwrap_or(open);
            path = Some(dir.join(code[open..end].iter().collect::<String>()));
        }
        j = (close + 1).min(n);
    }
}

/// The name a `mod name;` at `j` declares, past its `pub`, `pub(crate)`,
/// `pub(super)` or `pub(in ..)`; `None` for anything else, a
/// `mod name { .. }` block among them.
fn declared_mod(bare: &[char], j: usize) -> Option<String> {
    let mut j = past_visibility(bare, j);
    if !starts(bare, j, "mod") || !bare.get(j + 3).is_some_and(|c| c.is_whitespace()) {
        return None;
    }
    j = skip_space(bare, j + 3);
    let name: String = bare[j..].iter().take_while(|c| is_ident(**c)).collect();
    j = skip_space(bare, j + name.chars().count());
    (!name.is_empty() && bare.get(j) == Some(&';')).then_some(name)
}

/// Where the item at `j` starts past its `pub`, `pub(crate)`,
/// `pub(super)` or `pub(in ..)`.
fn past_visibility(bare: &[char], mut j: usize) -> usize {
    let n = bare.len();
    if starts(bare, j, "pub") {
        j += 3;
        if j < n && bare[j] == '(' {
            j = (j..n).find(|&k| bare[k] == ')').map_or(n, |k| k + 1);
        }
        j = skip_space(bare, j);
    }
    j
}

/// The file of module `name` declared from `home`: `name.rs`, else
/// `name/mod.rs`.
fn module_file(home: &Path, name: &str) -> PathBuf {
    let flat = home.join(format!("{name}.rs"));
    if flat.exists() {
        flat
    } else {
        home.join(name).join("mod.rs")
    }
}

/// Every `.rs` file under `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(next) = dirs.pop() {
        for entry in std::fs::read_dir(&next).expect("a directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The first index from `j` of `chars` that is not whitespace.
fn skip_space(chars: &[char], mut j: usize) -> usize {
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    j
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `chars` holds `word` at `at`.
fn starts(chars: &[char], at: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    chars.len() >= at + w.len() && chars[at..at + w.len()] == w[..]
}

/// Where `word` occurs in `chars`, in order.
fn occurrences(chars: &[char], word: &str) -> Vec<usize> {
    let w: Vec<char> = word.chars().collect();
    (0..(chars.len() + 1).saturating_sub(w.len()))
        .filter(|&i| chars[i..i + w.len()] == w[..])
        .collect()
}

/// Blanks `[from, to)` of `v`, keeping its newlines.
fn blank(v: &mut [char], from: usize, to: usize) {
    for c in &mut v[from..to] {
        if *c != '\n' {
            *c = ' ';
        }
    }
}

/// The two views of one file's text: `(code, bare)`.
pub fn views(text: &str) -> (String, String) {
    let (mut code, mut bare) = lexed(text);
    blank_test_items(&mut code, &mut bare);
    (code.into_iter().collect(), bare.into_iter().collect())
}

/// `text` with its comments blanked (`code`), and with its string and char
/// literal contents blanked as well (`bare`), as char vectors of the text's
/// length. Test items are still in place; `views` blanks them.
pub fn lexed(text: &str) -> (Vec<char>, Vec<char>) {
    let chars: Vec<char> = text.chars().collect();
    let mut code = chars.clone();
    let mut bare = chars.clone();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && matches!(next, Some('/' | '*')) {
            let end = comment_end(&chars, i);
            blank(&mut code, i, end);
            blank(&mut bare, i, end);
            i = end;
        } else if c == 'r' && raw_may_open(&chars, i) && matches!(next, Some('"' | '#')) {
            match raw_string(&chars, i) {
                Some((open, end, after)) => {
                    blank(&mut bare, open, end);
                    i = after;
                }
                None => i += 1,
            }
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
    (code, bare)
}

/// Where the comment opening at `i` (`//` or `/*`) ends (exclusive): at its
/// newline, or past the `*/` that closes it (block comments nest).
fn comment_end(chars: &[char], i: usize) -> usize {
    let n = chars.len();
    if chars.get(i + 1) == Some(&'/') {
        return (i..n).find(|&j| chars[j] == '\n').unwrap_or(n);
    }
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
    j
}

/// Whether the `r` at `i` may open a raw string: it starts a token, or
/// follows a `b` that does (`br"..."`).
fn raw_may_open(chars: &[char], i: usize) -> bool {
    i == 0 || !is_ident(chars[i - 1]) || (chars[i - 1] == 'b' && (i < 2 || !is_ident(chars[i - 2])))
}

/// The raw string `r"..."` or `r#"..."#` whose `r` is at `i`: where its
/// contents start and end (exclusive), and the index past its closing
/// hashes. `None` for a raw identifier `r#name`.
fn raw_string(chars: &[char], i: usize) -> Option<(usize, usize, usize)> {
    let n = chars.len();
    let hashes = (i + 1..n).take_while(|&j| chars[j] == '#').count();
    let open = i + 1 + hashes;
    if chars.get(open) != Some(&'"') {
        return None;
    }
    let end = (open + 1..n)
        .find(|&j| chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')))
        .unwrap_or(n);
    Some((open + 1, end, (end + 1 + hashes).min(n)))
}

/// Blanks each `#[cfg(test)]` through the end of the item it carries, in
/// both views. Sequential, so an attribute inside a blanked item is gone
/// before it is looked for.
fn blank_test_items(code: &mut [char], bare: &mut [char]) {
    let attr: Vec<char> = CFG_TEST.chars().collect();
    let n = bare.len();
    let mut i = 0;
    while i + attr.len() <= n {
        if bare[i..i + attr.len()] != attr[..] {
            i += 1;
            continue;
        }
        let end = item_end(bare, i + attr.len());
        blank(code, i, end);
        blank(bare, i, end);
        i = end.max(i + attr.len());
    }
}

/// The words an item or a statement opens with. A field, a variant or a
/// match arm opens with a name or a pattern instead, and a `,` ends it.
const ITEM_WORDS: &[&str] = &[
    "fn",
    "pub",
    "mod",
    "use",
    "struct",
    "enum",
    "union",
    "impl",
    "trait",
    "type",
    "const",
    "static",
    "let",
    "unsafe",
    "async",
    "extern",
    "macro_rules",
];

/// Where the item an attribute carries ends (exclusive), walking `bare`
/// from `from`, the end of the attribute: after the `;` of a declaration or
/// a statement, after the brace that closes a block item (a `let` waits for
/// its `;`); after the `,` of a field, a variant or a match arm, or before
/// the brace that closes their holder when they are its last. Braces,
/// parentheses and brackets nest, so a `,` inside them is not an end, and
/// an item's own commas (`fn f<A, B>(a, b)`) are passed because it opens
/// with an item word.
fn item_end(bare: &[char], from: usize) -> usize {
    let n = bare.len();
    // The word the item opens with, past any further attributes.
    let mut j = skip_space(bare, from);
    while starts(bare, j, "#[") {
        j = (j..n).find(|&k| bare[k] == ']').map_or(n, |k| k + 1);
        j = skip_space(bare, j);
    }
    let word: String = bare[j.min(n)..]
        .iter()
        .take_while(|c| is_ident(**c))
        .collect();
    let item = ITEM_WORDS.contains(&word.as_str());
    let statement = word == "let";
    let mut depth = 0usize;
    while j < n {
        match bare[j] {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                if depth == 0 {
                    // The last field, variant or arm of its holder.
                    return j;
                }
                depth -= 1;
                if depth == 0 && bare[j] == '}' && !statement {
                    return j + 1;
                }
            }
            ';' if depth == 0 => return j + 1,
            ',' if depth == 0 && !item => return j + 1,
            _ => {}
        }
        j += 1;
    }
    n
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
            .map(|(c, b)| if c == b { ' ' } else { c })
            .collect()
    }

    /// Whether the line is a `use` declaration.
    pub fn is_use(&self) -> bool {
        self.bare.split_whitespace().any(|w| w == "use")
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

/// How many times `needle` occurs in `line`.
pub fn count(line: &str, needle: &str) -> usize {
    line.matches(needle).count()
}

/// How many times `word` stands alone in `line`, an identifier boundary on
/// both sides: `use tokio::task::{spawn as go}` names `spawn` once,
/// `spawn_blocking` not at all.
pub fn words(line: &str, word: &str) -> usize {
    line.match_indices(word)
        .filter(|(at, _)| {
            let before = !line[..*at].chars().next_back().is_some_and(is_ident);
            let after = !line[at + word.len()..].chars().next().is_some_and(is_ident);
            before && after
        })
        .count()
}

/// How many times `line` calls `name` (a name with its `(`, as `unwrap(`),
/// at an identifier boundary: `x.unwrap(`, `Option::unwrap(` and a bare
/// `unwrap(` (a name brought in by a `use`) each count once; `my_unwrap(`
/// does not, nor does the definition `fn unwrap(`.
pub fn calls(line: &str, name: &str) -> usize {
    line.match_indices(name)
        .filter(|(at, _)| {
            let before = &line[..*at];
            let boundary = !before.chars().next_back().is_some_and(is_ident);
            let definition = before
                .trim_end()
                .strip_suffix("fn")
                .is_some_and(|d| !d.ends_with(is_ident));
            boundary && !definition
        })
        .count()
}

/// How many times the line calls any of `names` (each with its `(`, by
/// `calls`).
pub fn calls_any(line: &Line, names: &[&str]) -> usize {
    names
        .iter()
        .map(|n| calls(line.bare, &format!("{n}(")))
        .sum()
}

/// How many of `names` a `use` line brings in (by `words`); 0 on any other
/// line.
pub fn imports(line: &Line, names: &[&str]) -> usize {
    if line.is_use() {
        names.iter().map(|n| words(line.bare, n)).sum()
    } else {
        0
    }
}

/// One rule of docs/ARCH-RULES.md as data: its message, the files it never
/// reads (its homes), its allowlist `(file, today's count, why)`, and the
/// sites it counts on a line.
pub struct Rule {
    pub name: &'static str,
    pub homes: &'static [&'static str],
    pub allowed: &'static [(&'static str, usize, &'static str)],
    pub sites: fn(&Line) -> usize,
}

impl Rule {
    /// Counts the rule's sites outside its homes and judges them against
    /// its allowlist.
    pub fn check(&self) {
        let found = hits(|l| {
            if self.homes.contains(&l.source.file.as_str()) {
                0
            } else {
                (self.sites)(l)
            }
        });
        judge(self.name, &found, self.allowed);
    }
}
