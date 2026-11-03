#!/usr/bin/env python3
"""The Nightly's plan: which floored files a night mutates (#462).

Usage:
  changed_set.py --mode incremental|complete [--record <mutants.json>]
                 [--head <sha>] [--paths <file>] [--out <plan.json>]
                 [--github-output <file>] [--complete-shards <n>] [--max-shards <n>]

The Nightly has two modes. A COMPLETE night mutates every floored file (the
files of MUTANTS_FLOORS in .github/workflows/nightly.yml) in
MUTANTS_SHARDS_COMPLETE shards (`--complete-shards`, twenty: the account's
concurrent-job limit, #460), fourteen at a time, then six (COMPLETE_PARALLEL,
#496), the night of record cut at each pin (docs/RELEASING.md). An
INCREMENTAL night, the default on the captain's dispatch, mutates only the
floored files whose figure may have moved since the last complete run, and
the score carries every other file's figure from that run with its run and
sha (.github/nightly/mutants_score.py).

The last complete run is read from the record (docs/scoreboard/mutants.json):
the record itself when its `mode` is "complete", its `base` ({"run", "commit"})
when its mode is "incremental". The record must name its mode. The changed
paths are `git diff --name-only <base commit> <head>` (a tree diff, so the
base need not be an ancestor), or the lines of `--paths` (the captured input
the planted cases give). A path's effect on the set:

- a floored file: that file;
- the whole set: `Cargo.lock`, a workspace manifest (`Cargo.toml`,
  `mcp/Cargo.toml`), a toolchain or cargo config file, `build.rs`, the
  Nightly's own shape (`.github/workflows/nightly.yml`, which holds the
  floors, the oracle's selection and the cargo-mutants pin) and the left-out
  list (`.github/nightly/no-floored-targets`): a dependency bump counts as
  changing every file that uses it (the keeper's risk 0.2);
- a test (the keeper's risk 0.2: a test change reaches its files): a target
  under `tests/` or `benches/`, a file under one target's directory, or a
  shared input of the targets (any other file under `tests/`, `benches/common/`,
  and `fuzz/src/` and `fuzz/corpus/`, which `tests/fuzz_replay.rs` compiles
  and replays). A target the reach record (.github/nightly/no-floored-reach.json,
  #401) measures reaches the floored files it ran a line of, so a left-out
  target reaches none; a target the record does not measure, and a shared
  input, reach every floored file, since no record names their reach per file;
- another file under `src/` (#464's review), read by the crate's module tree
  (each crate root's `mod` declarations, at their `#[path]`, never a name
  guessed from a directory):
  - test-only code (a module declared under `#[cfg(test)]`, itself or an
    ancestor, in every crate that compiles it): the floored file whose module
    tree holds it (`src/<name>/**` for `src/<name>.rs`, or a `#[path]` module
    of it), and every floored file a unit test that can run its lines can run
    (a test in a floored file's module tree counting as that file's). A test can run a module's lines when the test's file names it,
    through the files it names in turn (`mod`, `use`, a `use ...::*`, a path
    `a::b`, resolved from `crate`, `super`, `self`, the lib's name, an alias
    or the file's module and its ancestors); so a change to
    `src/spend_gate/tests.rs` reaches every floored file its tests and the
    tests that name it reach by name;
  - any other code under `src/` (a module the lib or a binary compiles
    outside its tests, a crate root, or a file none of the trees holds): the
    whole set. A unit test anywhere in the lib or a binary (`--lib --bins`,
    in every shard) and every test target may run its lines beside a floored
    file's, and no record measures a unit test's reach per file; this is the
    conservative choice, and its cost is that a night whose diff touches such
    a file is a complete-sized night;
  - a file under `src/` that a test target, a bench or `fuzz/src/` compiles
    by `#[path]` is also that target's input (above), and a file a source
    file reads by `include_str!` or `include_bytes!` has that source file's
    effect; any other non-Rust file under `src/` (its notes): nothing;
- anything else (the docs, the tools, `benches/baseline.json`, the fuzz
  crate's own manifest and lock): nothing.

The plan names the selection the shards pass `cargo test` (`--lib --bins`, a
`--test` for every target under tests/ and a `--bench` for every bench, less
the left-out list's entries: the oracle complete per floored file, the owner's
23.9 word), the shard count, the matrix and the matrix's max-parallel. A
complete night: `--complete-shards` shards (#460), COMPLETE_PARALLEL at once
(#496: the account's concurrent-job limit less a PR run's six jobs, so twenty
shards run in two waves, fourteen then six). An incremental night keeps its own
sizing (#462): a whole set `--max-shards` (MUTANTS_SHARDS_INCREMENTAL);
otherwise one shard per PER_SHARD of the changed files' mutants, by the
record's counts, and at least PARALLEL while there are that many mutants, at
most `--max-shards`; PARALLEL (six) at a time. An empty changed set plans no
shard: the night carries every figure and says so.

Exit 1, naming the fault, on a record without a mode or a base, a mode other
than the two, or a git diff that fails.
"""

import argparse
import json
import math
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import no_floored  # noqa: E402  (the one reader of the floors and the left-out list)

ROOT = HERE.parent.parent
RECORD = "docs/scoreboard/mutants.json"
REACH = ".github/nightly/no-floored-reach.json"
MODES = ("incremental", "complete")
# An incremental night's sizing (#462): 19 mutants at most a shard, the
# complete night's of the record then (923 in fifty); a complete night is
# --complete-shards since #460, twenty, 47 a shard.
PER_SHARD = 19
# A complete night's count (#460): the account's concurrent-job limit.
COMPLETE_SHARDS = 20
# A complete night's max-parallel (#496; the keeper's fleet line of 06:2xZ
# 11-03): the account's twenty runners are one pool across four repositories,
# and a PR run's six jobs (lint, Build, Test, vectors, wire, e2e-hermetic;
# `changes` is seconds) need six of them. Night 3 (run 41516017523) held all
# twenty and the PR runs of 11-03 took 47 to 55 minutes for 5 minutes of work.
# So twenty shards run fourteen, then six; 14 until a later row reads the
# limit from the API.
ACCOUNT_RUNNERS = 20
PR_RUN_JOBS = 6
COMPLETE_PARALLEL = ACCOUNT_RUNNERS - PR_RUN_JOBS
# An incremental night's max-parallel (#144): a small set still fills the six runners a
# night may hold, since a shard's wall clock is its baseline (about 11 minutes
# in run 41516017523) and its mutants, not their count across shards.
PARALLEL = 6
WHOLE = {
    "Cargo.lock": "the lockfile changed: a dependency counts as changing every file that uses it",
    "Cargo.toml": "a workspace manifest changed: a dependency counts as changing every file that uses it",
    "mcp/Cargo.toml": "a workspace manifest changed: a dependency counts as changing every file that uses it",
    "rust-toolchain": "the toolchain changed",
    "rust-toolchain.toml": "the toolchain changed",
    ".cargo/config": "the cargo config changed",
    ".cargo/config.toml": "the cargo config changed",
    ".cargo/mutants.toml": "cargo-mutants' config changed",
    "build.rs": "the build script changed",
    ".github/workflows/nightly.yml": "the Nightly's shape changed (the floors, the selection, the cargo-mutants pin)",
    ".github/nightly/no-floored-targets": "the oracle's left-out list changed",
}
SHA = re.compile(r"^[0-9a-f]{40}$")
LIB = "bsv_wallet_cli"
# Comments, strings and char literals, in one pass (a `#[path]`'s string is
# kept first, as `#[path=<..>]`).
TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|b?r(#*)".*?"\1|b?"(?:\\.|[^"\\])*"|b?\'(?:\\.|[^\'\\])\'', re.S)
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
PATH_KEPT = re.compile(r"#\[path=<([^>]+)>\]")
MOD = re.compile(r"((?:#\[[^\]]*\]\s*)*)(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;")
CFG_TEST = re.compile(r"#\[cfg\(\s*test\s*\)\]")
USE = re.compile(r"\buse\s+([^;]+);")
NAME_PATH = re.compile(r"[A-Za-z_]\w*(?:\s*::\s*(?:[A-Za-z_]\w*|\*))+")
TEST_FN = re.compile(r"#\[(?:[\w:]+::)?test\b")
INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')


def fault(why):
    sys.exit(f"changed_set: {why}")


def last_complete(record):
    """The last complete run, {"run", "commit"}, as the record names it."""
    mode = record.get("mode")
    if mode not in MODES:
        fault(f"{RECORD} names no mode (complete or incremental); the last complete run is unknown")
    base = {"run": record.get("run"), "commit": record.get("commit")} if mode == "complete" else record.get("base")
    if not isinstance(base, dict) or not isinstance(base.get("run"), str) or not SHA.match(str(base.get("commit"))):
        fault(f"{RECORD}: the last complete run is not named (a run URL and a 40-hex commit)")
    return {"run": base["run"], "commit": base["commit"]}


def targets(root):
    """Every test target and bench of the package, as (kind, name)."""
    out = [("test", p.stem) for p in sorted((root / "tests").glob("*.rs"))]
    out += [("test", p.parent.name) for p in sorted((root / "tests").glob("*/main.rs"))]
    out += [("bench", p.stem) for p in sorted((root / "benches").glob("*.rs"))]
    return out


def reach_map(root, files):
    """{(kind, name): the floored files it ran a line of}, for each target the record measures."""
    path = root / REACH
    if not path.exists():
        return {}
    record = json.loads(path.read_text())
    return {(t["kind"], t["name"]): {f for f in files if t.get("reach", {}).get(f)}
            for t in record.get("targets", [])}


def target_of(path, root):
    """The (kind, name) a test path belongs to, or None for a shared input."""
    parts = path.split("/")
    if parts[0] == "tests" and len(parts) == 2 and path.endswith(".rs"):
        return ("test", parts[1][:-3])
    if parts[0] == "tests" and len(parts) > 2 and (root / "tests" / parts[1] / "main.rs").is_file():
        return ("test", parts[1])
    if parts[0] == "benches" and len(parts) == 2 and path.endswith(".rs"):
        return ("bench", parts[1][:-3])
    return None


def is_test_input(path):
    return (path.startswith("tests/")
            or path.startswith("benches/common/") or (path.startswith("benches/") and path.endswith(".rs"))
            or path.startswith("fuzz/src/") or path.startswith("fuzz/corpus/"))


def strip(text):
    """Rust source without its comments, strings and char literals."""
    text = PATH_ATTR.sub(lambda m: f"#[path=<{m.group(1)}>]", text)
    return TOKEN.sub(lambda m: "" if m.group(0).startswith("/") else '""', text)


def expand(tree):
    """A use tree's paths: [(segments, the name it binds)]."""
    out = []

    def go(prefix, t):
        t = t.strip()
        if not t:
            return
        i = t.find("{")
        if i >= 0:
            head = [s.strip() for s in t[:i].split("::") if s.strip()]
            depth, parts, cur = 0, [], ""
            for ch in t[i + 1:t.rindex("}")]:
                depth += (ch == "{") - (ch == "}")
                if ch == "," and depth == 0:
                    parts.append(cur)
                    cur = ""
                else:
                    cur += ch
            for part in parts + [cur]:
                go(prefix + head, part)
            return
        alias = None
        if " as " in t:
            t, alias = (s.strip() for s in t.split(" as ", 1))
        segs = prefix + [s.strip() for s in t.split("::") if s.strip()]
        if segs and segs[-1] == "self":
            segs = segs[:-1]
        if segs:
            out.append((segs, alias or segs[-1]))

    go([], re.sub(r"\s+", " ", tree))
    return out


class Crates:
    """The package's module trees, read from each crate root's `mod`
    declarations: the lib (src/lib.rs), the binary src/main.rs and each
    src/bin/*.rs. A module is (crate, path); a file may be compiled into more
    than one crate."""

    def __init__(self, root):
        self.root = root
        self.mods = {}  # (crate, path) -> file
        self.places = {}  # file -> [(crate, path)]
        self.test_only = {}  # (crate, path) -> declared under #[cfg(test)]
        self.text = {}
        roots = {"lib": root / "src/lib.rs", "main": root / "src/main.rs"}
        roots.update({f"bin:{b.stem}": b for b in sorted((root / "src/bin").glob("*.rs"))})
        for crate, f in roots.items():
            if f.is_file():
                self.walk(crate, (), f, False)
        self.edges = {f: self.names(f) for f in self.places}
        self.tests = {f for f in self.places if TEST_FN.search(self.text[f])}

    def rel(self, f):
        return f.resolve().relative_to(self.root.resolve()).as_posix()

    def walk(self, crate, path, f, test):
        rel = self.rel(f)
        if (crate, path) in self.mods:
            return
        self.mods[(crate, path)] = rel
        self.places.setdefault(rel, []).append((crate, path))
        self.test_only[(crate, path)] = test
        self.text[rel] = text = strip(f.read_text())
        own = f.parent if (not path or f.name == "mod.rs") else f.with_suffix("")
        for attrs, name in MOD.findall(text):
            kept = PATH_KEPT.search(attrs)
            for c in ([f.parent / kept.group(1)] if kept else [own / f"{name}.rs", own / name / "mod.rs"]):
                if c.is_file():
                    self.walk(crate, path + (name,), c, test or bool(CFG_TEST.search(attrs)))
                    break

    def resolve(self, segs, crate, path, aliases, depth=0):
        """The modules a path names: the longest prefix that is a module."""
        head = segs[0]
        if head == "crate":
            bases, rest = [(crate, ())], segs[1:]
        elif head == LIB:
            bases, rest = [("lib", ())], segs[1:]
        elif head == "self":
            bases, rest = [(crate, path)], segs[1:]
        elif head == "super":
            k = 0
            while k < len(segs) and segs[k] == "super":
                k += 1
            # The file's module, or an inline module in it (`mod tests { use super::*; }`).
            bases, rest = [(crate, path[:len(path) - k]), (crate, path[:len(path) - k + 1])], segs[k:]
        elif head in aliases and depth < 8:
            return self.resolve(aliases[head] + segs[1:], crate, path, aliases, depth + 1)
        else:
            # A child of the file's module, or of an ancestor (a glob import's reach).
            bases, rest = [(crate, path[:i]) for i in range(len(path), -1, -1)], segs
        if not rest:
            return {b for b in bases if b in self.mods}
        found = set()
        for c, base in bases:
            best = None
            for k in range(1, len(rest) + 1):
                if (c, base + tuple(rest[:k])) in self.mods:
                    best = (c, base + tuple(rest[:k]))
            if best:
                found.add(best)
                if head != "super":
                    break
        return found

    def names(self, f):
        """The files a file names, in every crate that compiles it."""
        text = self.text[f]
        uses = [p for u in USE.findall(text) for p in expand(u)]
        named = set()
        for crate, path in self.places[f]:
            aliases = {alias: segs for segs, alias in uses if segs[-1] != "*"}
            for segs, _ in uses:
                named |= self.resolve([s for s in segs if s != "*"] or ["self"], crate, path, {})
            for m in NAME_PATH.findall(text):
                segs = [s.strip() for s in m.split("::") if s.strip() != "*"]
                named |= self.resolve(segs, crate, path, aliases)
            for _, name in MOD.findall(text):
                if (crate, path + (name,)) in self.mods:
                    named.add((crate, path + (name,)))
        return {self.mods[m] for m in named} - {f}

    def closure(self, f):
        seen, todo = {f}, [f]
        while todo:
            for g in self.edges[todo.pop()]:
                if g not in seen:
                    seen.add(g)
                    todo.append(g)
        return seen

    def owners(self, f, floored):
        """The floored files whose module tree holds f."""
        out = set()
        for crate, path in self.places.get(f, []):
            for fl in floored:
                if any(c == crate and path[:len(p)] == p for c, p in self.places.get(fl, [])):
                    out.add(fl)
        return out

    def is_test_only(self, f):
        return f in self.places and all(self.test_only[p] for p in self.places[f])

    def test_reach(self, f, floored):
        """The floored files a unit test that can run f's lines can run."""
        out = self.owners(f, floored)
        for t in self.tests:
            seen = self.closure(t)
            if f in seen:  # A test in a floored file's module tree is that file's test (above).
                out |= (seen & set(floored)) | self.owners(t, floored)
        return out

    def includes(self):
        """{a non-Rust file under src/: the source files that read it by include_str!/include_bytes!}."""
        out = {}
        for f in self.places:
            raw = (self.root / f).read_text()
            for name in INCLUDE.findall(raw):
                out.setdefault(self.rel((self.root / f).parent / name), set()).add(f)
        return out


def compiled_by_targets(root):
    """{a file under src/: the test, bench or fuzz files that compile it by #[path]}."""
    out = {}
    for top in ("tests", "benches", "fuzz/src"):
        for f in sorted((root / top).rglob("*.rs")):
            for name in PATH_ATTR.findall(f.read_text()):
                p = (f.parent / name).resolve()
                if p.is_file() and p.is_relative_to((root / "src").resolve()):
                    out.setdefault(p.relative_to(root.resolve()).as_posix(), set()).add(
                        f.relative_to(root).as_posix())
    return out


def test_effect(path, every, reach, root):
    """A test path's effect: (files, why)."""
    target = target_of(path, root)
    if target is None:
        return every, "a shared input of the test targets: no record names its reach per file"
    if target in reach:
        hit = reach[target]
        return hit, (f"{target[0]} {target[1]}: it reaches "
                     + (", ".join(sorted(hit)) if hit else "no floored file (the reach record, #401)"))
    return every, f"{target[0]} {target[1]}: the reach record does not measure it"


def source_effect(path, every, crates):
    """A src/ path's effect (not a floored file): (files, why)."""
    tree = {f for f in every if path.startswith(f[:-len(".rs")] + "/")}
    hit, why = source_reach(path, every, crates)
    if tree - hit:
        why = f"in the module directory of {', '.join(sorted(tree))}; {why}"
    return hit | tree, why


def source_reach(path, every, crates):
    if path in crates.places:
        if crates.is_test_only(path):
            hit = crates.test_reach(path, every)
            return hit, ("test-only code: the floored files a unit test that can run it can run, by name"
                         if hit else "test-only code that no unit test names")
        return every, ("code the lib or a binary compiles outside its tests: a unit test or a test target "
                       "may run it beside any floored file (the conservative choice)")
    readers = crates.includes().get(path)
    if readers:
        hit = set().union(*(source_effect(r, every, crates)[0] if r not in every else {r} for r in sorted(readers)))
        return hit, f"read by {', '.join(sorted(readers))} (include_str!/include_bytes!)"
    if path.endswith(".rs"):
        return every, "a source file no crate's module tree holds (the conservative choice)"
    return set(), "reaches no floored file"


def effects(paths, floored, reach, root):
    """Each changed path's effect: [(path, files it brings in, why)]."""
    out = []
    every = set(floored)
    crates = compiled = None
    for path in paths:
        if path in floored:
            out.append((path, {path}, "a floored file"))
        elif path in WHOLE:
            out.append((path, every, WHOLE[path]))
        elif is_test_input(path):
            hit, why = test_effect(path, every, reach, root)
            out.append((path, hit, why))
        elif path.startswith("src/"):
            if crates is None:
                crates, compiled = Crates(root), compiled_by_targets(root)
            hit, why = source_effect(path, every, crates)
            for by in sorted(compiled.get(path, ())):
                more, how = test_effect(by, every, reach, root) if not by.startswith("fuzz/") else (
                    every, "a shared input of the test targets: no record names its reach per file")
                hit, why = hit | more, f"{why}; compiled by {by} (#[path]): {how}"
            out.append((path, hit, why))
        else:
            out.append((path, set(), "reaches no floored file"))
    return out


def selection(root):
    left_out = set(no_floored.entries(root)[0])
    words = ["--lib", "--bins"]
    for kind, name in targets(root):
        if (kind, name) not in left_out:
            words.append(f"--{kind} {name}")
    return " ".join(words)


def mutant_count(record, file):
    entry = record.get("files", {}).get(file)
    if not entry:
        return PER_SHARD * 5  # An unrecorded file: five shards' worth.
    return sum(entry.get(k, 0) for k in ("caught", "missed", "timeout", "unviable"))


def plan(mode, record, paths, root, head, max_shards, complete_shards=COMPLETE_SHARDS):
    floored = no_floored.floored(root)
    base = last_complete(record)
    if mode == "complete":
        found, files = [], set(floored)
    else:
        found = effects(paths, floored, reach_map(root, floored), root)
        files = set().union(*(f for _, f, _ in found)) if found else set()
    files = sorted(files)
    whole = files == sorted(floored)
    mutants = sum(mutant_count(record, f) for f in files)
    if not files:
        shards = 0
    elif mode == "complete":
        shards = complete_shards
    elif whole:
        shards = max_shards
    else:
        shards = min(max_shards, max(1, math.ceil(mutants / PER_SHARD), min(PARALLEL, mutants)))
    # Fourteen at a time on a complete night, a PR run's six runners left (#496);
    # six at a time on an incremental one (#144); never more than the count.
    parallel = max(1, min(COMPLETE_PARALLEL if mode == "complete" else PARALLEL, shards))
    return {
        "mode": mode,
        "base": base,
        "head": head,
        "floored": floored,
        "files": files,
        "carried": [f for f in floored if f not in files],
        "whole": whole,
        "mutants": mutants,
        "shards": shards,
        "matrix": list(range(shards)),
        "parallel": parallel,
        "selection": selection(root),
        "paths": [{"path": p, "files": sorted(f), "why": why} for p, f, why in found],
    }


def brings(files, floored):
    return "every floored file" if files == floored else ", ".join(f"`{f}`" for f in files)


def waves(p):
    n, k = p["shards"], p["parallel"]
    full, rest = divmod(n, k)
    sizes = [k] * full + ([rest] if rest else [])
    return f"{len(sizes)} wave{'s' if len(sizes) != 1 else ''}: {', then '.join(map(str, sizes))}"


def summary(p):
    lines = [f"## The night's plan ({p['mode']})", ""]
    lines.append(f"The last complete run: {p['base']['run']} at `{p['base']['commit']}`; the night's tree: `{p['head']}`.")
    lines.append("")
    if p["mode"] == "complete":
        lines.append(f"A complete night: every floored file, {p['mutants']} mutants by the record, in {p['shards']} shards, "
                     f"{p['parallel']} at once ({waves(p)}).")
    elif not p["files"]:
        lines.append("No floored file changed and no test or dependency reaches one: the night mutates nothing "
                     "and carries every file's figure from the last complete run.")
    else:
        lines.append(f"Mutated: {', '.join(f'`{f}`' for f in p['files'])} ({p['mutants']} mutants by the record, "
                     f"{p['shards']} shards, {p['parallel']} at once){'; the whole set' if p['whole'] else ''}.")
        lines.append(f"Carried from the last complete run: {', '.join(f'`{f}`' for f in p['carried']) or 'none'}.")
    reaching = [e for e in p["paths"] if e["files"]]
    if reaching:
        lines += ["", "| changed path | brings in | why |", "|---|---|---|"]
        lines += [f"| `{e['path']}` | {brings(e['files'], p['floored'])} | {e['why']} |" for e in reaching]
    quiet = len(p["paths"]) - len(reaching)
    if quiet:
        lines += ["", f"{quiet} other changed path(s) reach no floored file."]
    return "\n".join(lines) + "\n"


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--mode", required=True)
    ap.add_argument("--record", default=None)
    ap.add_argument("--head", default=None)
    ap.add_argument("--paths", default=None, help="the changed paths, one a line (else git diff)")
    ap.add_argument("--out", default=None)
    ap.add_argument("--github-output", default=None)
    ap.add_argument("--root", default=None)
    ap.add_argument("--max-shards", type=int, default=50, help="an incremental night's ceiling")
    ap.add_argument("--complete-shards", type=int, default=COMPLETE_SHARDS, help="a complete night's count")
    args = ap.parse_args(argv)
    root = Path(args.root).resolve() if args.root else ROOT
    if args.mode not in MODES:
        fault(f"mode {args.mode!r} is not incremental or complete")
    record = json.loads((Path(args.record) if args.record else root / RECORD).read_text())
    head = args.head or subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True,
                                       text=True).stdout.strip()
    if args.mode == "complete":
        paths = []
    elif args.paths:
        paths = [l.strip() for l in Path(args.paths).read_text().splitlines() if l.strip()]
    else:
        base = last_complete(record)["commit"]
        diff = subprocess.run(["git", "diff", "--name-only", base, head], cwd=root, capture_output=True, text=True)
        if diff.returncode != 0:
            fault(f"git diff {base} {head} failed: {diff.stderr.strip()}")
        paths = [l for l in diff.stdout.splitlines() if l]
    if args.complete_shards < 1 or args.max_shards < 1:
        fault(f"a shard count is at least 1 (--complete-shards {args.complete_shards}, --max-shards {args.max_shards})")
    p = plan(args.mode, record, paths, root, head, args.max_shards, args.complete_shards)
    text = summary(p)
    sys.stdout.write(text)
    if args.out:
        Path(args.out).write_text(json.dumps(p, indent=2) + "\n")
    if args.github_output:
        with open(args.github_output, "a") as out:
            out.write(f"mode={p['mode']}\n")
            out.write(f"files={' '.join(p['files'])}\n")
            out.write(f"shards={p['shards']}\n")
            out.write(f"matrix={json.dumps(p['matrix'])}\n")
            out.write(f"parallel={p['parallel']}\n")
            out.write(f"empty={'true' if not p['files'] else 'false'}\n")
            out.write(f"selection={p['selection']}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
