#!/usr/bin/env python3
"""The Nightly's plan: which floored files a night mutates (#462).

Usage:
  changed_set.py --mode incremental|complete [--record <mutants.json>]
                 [--head <sha>] [--paths <file>] [--out <plan.json>]
                 [--github-output <file>] [--complete-shards <n>] [--max-shards <n>]
                 [--cap 6|14] [--window-end HH:MMZ] [--now <ISO time>] [--fuzz-seconds <n>]

The Nightly has two modes. A COMPLETE night mutates every floored file (the
files of MUTANTS_FLOORS in .github/workflows/nightly.yml) in
MUTANTS_SHARDS_COMPLETE shards (`--complete-shards`, twenty: the account's
concurrent-job limit, #460), six at a time in four waves (HEAVY_RUN_PARALLEL,
#513), the night of record cut at each pin (docs/RELEASING.md). An
INCREMENTAL night, the default on the captain's dispatch, mutates only the
floored files whose figure may have moved since the base (the last
base-setting run, below), and the score carries every other file's figure
from that run with its run and sha (.github/nightly/mutants_score.py).

The base, the last base-setting run, is read from the record
(docs/scoreboard/mutants.json): the record itself when its `mode` is
"complete"; its `base` ({"run", "commit"}) when its mode is "incremental".
An incremental night whose changed set was the whole set judged every mutant
of every floored file at one sha, a complete night by another shard count, so
it re-bases (#559): its record's `base` names its own run and commit with
`"whole": true` (`base: this run, the changed set the whole set`), and the
next night diffs against it. A night that judged a subset keeps the base it
carried from. A `whole` base that names another run than the record's, or a
base without it that names the record's own run, is refused. The plan names
the base it diffed against and why (`base_why`). The record must name its
mode. The changed paths are `git diff --name-only <base commit> <head>` (a
tree diff, so the
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
complete night: `--complete-shards` shards (#460), HEAVY_RUN_PARALLEL at once
(#513: every heavy run of the fleet caps at six while the account's runner
pool reads busy, so twenty shards run in four waves of six, the last two). An incremental night keeps its own
sizing (#462): a whole set `--max-shards` (MUTANTS_SHARDS_INCREMENTAL);
otherwise one shard per PER_SHARD of the changed files' mutants, by the
record's counts, and at least PARALLEL while there are that many mutants, at
most `--max-shards`; PARALLEL (six) at a time. An empty changed set plans no
shard: the night carries every figure and says so. Either night's stage after
the matrix (#517): the fuzz targets FUZZ_PARALLEL (three) at once beside the
hermetic shards HERMETIC_PARALLEL (three) at a time, then reach and bench, the
figures the plan's outputs `fuzz_parallel` and `hermetic_parallel`, refused
when their sum passes HEAVY_RUN_PARALLEL.

The cap (#584, the owner's word of 2026-11-04 and the keeper's windows table,
docs/NIGHTLY.md): `--cap`, the dispatch's input `cap`, is 6 (the default, the
figures above exactly) or 14. The matrix runs min(cap, shards) at once and the
stage after it splits the cap between the fuzz targets and the hermetic shards
(STAGE_SPLIT: three plus three under six, seven plus seven under fourteen).
The plan projects the night: the shard count, the measured shard time (the
record's runner minutes a mutant, from its shards' start on a runner to their
end, times the plan's mutants a shard), the waves, the stage after the matrix
(the fuzz rounds at `--fuzz-seconds` and the hermetic rounds at the record's
hermetic shard time) and reach, and the projected end from the plan's minute
(`--now`, else the clock). GitHub's own waits (a job queued behind the hosted
runners) are not projected: they are GitHub's, named as such in the record's
wall-clock row. With `--window-end` (the dispatch's `window_end`, the end of
the window the keeper's table gives the night), fourteen whose projected end
falls past the window's end, or that the record cannot project, is refused
with a line and the night falls back to six; the cap never exceeds the
table's word. Without a window there is no window check.

Exit 1, naming the fault, on a record without a mode or a base, a mode other
than the two, or a git diff that fails.
"""

import argparse
import datetime
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
# A complete night's max-parallel (#513; the keeper's fleet line of 10:0xZ
# 11-03, from the opening API read of the account's runner pool: 12,251 jobs
# over three days). The account's twenty runners are one pool across four
# repositories, and it sat at twenty at its peak in 22 of 24 hours; a job
# queued beside a heavy run waited a median 0:38 and a worst 88:53, against
# 0:02 and 3:16 beside none; the non-heavy jobs' p95 is 14 slots, so #496's
# 14 (20 less a PR run's six) beside them asked for 28 of 20. Every heavy run
# of the fleet caps at six: twenty shards run in four waves (6, 6, 6, 2).
# Raise it only when the next occupancy read (every 48 hours, fleet-wide)
# shows the non-heavy p95 under 10 slots.
HEAVY_RUN_PARALLEL = 6
# An incremental night's max-parallel (#144): a small set still fills the six runners a
# night may hold, since a shard's wall clock is its baseline (about 11 minutes
# in run 41516017523) and its mutants, not their count across shards. Six is
# already HEAVY_RUN_PARALLEL's cap (#513), so the incremental rule is unchanged.
PARALLEL = 6
# The stage after the matrix (#517), under the same cap. Once the cargo matrix
# ends, the fuzz targets and the hermetic shards run together, FUZZ_PARALLEL
# and HERMETIC_PARALLEL at once (3 and 3: the five targets, an hour each on a
# complete night, in two rounds, beside the six hermetic shards in two rounds
# of about 35 minutes), and the reach job and the bench wait for both
# (needs: in nightly.yml) and run last, two jobs. Before #517 the hermetic
# six, the five targets, reach and bench could run together, thirteen jobs.
# The split is the captain's call on the lane's figures from run 41516017523:
# five plus one ran the hermetic shards in file, 196 minutes, and grew the
# stage from about 1h15m to about 3h30m; three plus three runs it in about two
# hours, the fuzz targets' two rounds. The fuzz figure is the cap less the
# hermetic one, an explicit max-parallel so that a fourth target queues rather
# than slips past; the plan refuses a night whose figures sum past the cap
# (stage() below).
FUZZ_PARALLEL = 3
HERMETIC_PARALLEL = 3
# The last two jobs of the night (#517): reach and bench, after both.
TAIL_JOBS = 2
# The dispatch's cap (#584): six, the default and every figure above; or
# fourteen, the keeper's table's word for the CLI's 17:00Z to 20:00Z window
# (fourteen of the account's twenty slots), refused past the window's end.
CAPS = (HEAVY_RUN_PARALLEL, 14)
# The stage after the matrix per cap (#517, #584): fuzz plus hermetic = cap.
STAGE_SPLIT = {6: (FUZZ_PARALLEL, HERMETIC_PARALLEL), 14: (7, 7)}
# The projection's figures the record does not carry (#584), from night 4
# (run 41684822793, docs/NIGHTLY.md's wall clock): a fuzz target is its
# budget plus up to seven minutes (12 to 17 minutes at 600 s), the reach job
# eight minutes (the bench beside it, about seven), the five targets of the
# fuzz job's matrix, and at most MUTANTS_HERMETIC_SHARDS (six) hermetic shards.
FUZZ_TARGETS = 5
FUZZ_SETUP_MINUTES = 7
REACH_MINUTES = 8
HERMETIC_SHARDS = 6
WINDOW_END = re.compile(r"^([01]\d|2[0-3]):([0-5]\d)Z$")
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


def stage(cap=HEAVY_RUN_PARALLEL, fuzz=FUZZ_PARALLEL, hermetic=HERMETIC_PARALLEL, tail=TAIL_JOBS):
    """The stage after the matrix (#517): the fuzz targets beside the hermetic
    shards, then reach and bench; refused when a figure is below one or a
    step of the stage would run more than the cap at once."""
    if min(cap, fuzz, hermetic) < 1:
        fault(f"a parallel figure is at least 1 (cap {cap}, fuzz {fuzz}, hermetic {hermetic})")
    if fuzz + hermetic > cap:
        fault(f"the stage after the matrix runs fuzz {fuzz} + hermetic {hermetic} = {fuzz + hermetic} "
              f"jobs at once, over the cap of {cap} (#517)")
    if tail > cap:
        fault(f"the night's last jobs, reach and bench, are {tail}, over the cap of {cap} (#517)")
    return {"cap": cap, "fuzz_parallel": fuzz, "hermetic_parallel": hermetic, "tail": tail}


def base_of(record):
    """The last base-setting run, {"run", "commit"}, as the record names it, and why
    it is the base: a complete record is its own base; an incremental record whose
    changed set was the whole set names itself (`"whole": true`, #559); any other
    incremental record names the base it carried from."""
    mode = record.get("mode")
    if mode not in MODES:
        fault(f"{RECORD} names no mode (complete or incremental); the base is unknown")
    own = {"run": record.get("run"), "commit": record.get("commit")}
    base = own if mode == "complete" else record.get("base")
    if not isinstance(base, dict) or not isinstance(base.get("run"), str) or not SHA.match(str(base.get("commit"))):
        fault(f"{RECORD}: the base is not named (a run URL and a 40-hex commit)")
    named = {"run": base["run"], "commit": base["commit"]}
    if mode == "complete":
        return named, f"the record itself, a complete run ({run_id(named['run'])} at {named['commit'][:7]})"
    if base.get("whole") is True:
        if named != own:
            fault(f"{RECORD}: a whole base names the record's own run (#559), and this one names run "
                  f"{named['run']} at {named['commit']}, the record being run {own['run']} at {own['commit']}")
        return named, (f"the record itself, an incremental night whose changed set was the whole set "
                       f"({run_id(named['run'])} at {named['commit'][:7]}; base: this run, #559)")
    if "whole" in base:
        fault(f"{RECORD}: the base's `whole` is true or absent (#559), not {base['whole']!r}")
    if named == own:
        fault(f"{RECORD}: an incremental record names its own run as its base without `\"whole\": true` (#559)")
    return named, (f"the record's base, run {run_id(named['run'])} at {named['commit'][:7]}, which the record's "
                   f"night (run {run_id(str(own['run']))}) kept: a night that judged a subset keeps its base")


def last_complete(record):
    """The base {"run", "commit"} alone (base_of's first half)."""
    return base_of(record)[0]


def run_id(url):
    return url.rsplit("/", 1)[-1]


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


def plan(mode, record, paths, root, head, max_shards, complete_shards=COMPLETE_SHARDS, cap=HEAVY_RUN_PARALLEL):
    floored = no_floored.floored(root)
    base, base_why = base_of(record)
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
    # The dispatch's cap at a time (#584; six by default, the fleet's heavy-run
    # cap of #513 and an incremental night's six of #144); never more than the count.
    parallel = max(1, min(cap, shards))
    return {
        "mode": mode,
        "base": base,
        "base_why": base_why,
        "head": head,
        "floored": floored,
        "files": files,
        "carried": [f for f in floored if f not in files],
        "whole": whole,
        "mutants": mutants,
        "shards": shards,
        "matrix": list(range(shards)),
        "parallel": parallel,
        "cap": cap,
        "stage": stage(cap, *STAGE_SPLIT[cap]),
        "selection": selection(root),
        "paths": [{"path": p, "files": sorted(f), "why": why} for p, f, why in found],
    }


def minutes(start, end):
    t = [datetime.datetime.fromisoformat(x.replace("Z", "+00:00")) for x in (start, end)]
    return (t[1] - t[0]).total_seconds() / 60


def measured(record):
    """The record's measured times (#584): the newest night of the record and its
    earlier_nights whose every cargo shard names its start, end and mutants, as
    {"run", "rate" (runner minutes a mutant), "hermetic" (mean minutes a hermetic
    shard, or None), "hermetic_shards" (that night's count)}; None when no night
    carries them."""
    for night in [record] + list(record.get("earlier_nights", [])):
        shards = night.get("shards") or []
        if not shards or not all(isinstance(s, dict) and s.get("started") and s.get("completed")
                                 and isinstance(s.get("mutants"), int) for s in shards):
            continue
        total = sum(s["mutants"] for s in shards)
        if total < 1:
            continue
        rate = sum(minutes(s["started"], s["completed"]) for s in shards) / total
        hs = [minutes(s["started"], s["completed"]) for s in night.get("hermetic_shards") or []
              if isinstance(s, dict) and s.get("started") and s.get("completed")]
        return {"run": night.get("run"), "rate": rate, "hermetic": sum(hs) / len(hs) if hs else None,
                "hermetic_shards": len(hs)}
    return None


def hhmm(t):
    return t.strftime("%H:%MZ")


def window_end_after(now, end):
    """The next instant at `end` (HH:MMZ) after `now`."""
    m = WINDOW_END.match(end)
    if not m:
        fault(f"window_end {end!r} is not HH:MMZ (00:00Z to 23:59Z), or empty for no window check")
    t = now.replace(hour=int(m.group(1)), minute=int(m.group(2)), second=0, microsecond=0)
    return t if t > now else t + datetime.timedelta(days=1)


def project(p, rec, now, fuzz_seconds):
    """The night's projection at the plan's cap (#584): the matrix's waves of the
    measured shard time, then the stage after it, then reach; the end from `now`.
    Runner minutes alone: GitHub's own waits are GitHub's, never projected."""
    n, k, st = p["shards"], p["parallel"], p["stage"]
    out = {"cap": p["cap"], "shards": n, "parallel": k, "measured_run": rec["run"] if rec else None,
           "now": now.strftime("%Y-%m-%dT%H:%M:%SZ"), "github_waits": "not projected: GitHub's, named in the record"}
    if rec is None or (n and not p["mutants"]):
        out.update(shard_minutes=None, waves=None, matrix_minutes=None, stage_minutes=None, end=None)
        return out
    # The mean shard (round-robin deals the mutants evenly; the waves roll, a
    # slot refilled as a shard ends), so the matrix is its waves of the mean.
    per_shard = p["mutants"] / n if n else 0
    shard = rec["rate"] * per_shard
    waves_n = math.ceil(n / k) if n else 0
    matrix = waves_n * shard
    fuzz = math.ceil(FUZZ_TARGETS / st["fuzz_parallel"]) * (fuzz_seconds / 60 + FUZZ_SETUP_MINUTES)
    # A complete night's hermetic shards are six; an incremental night's, one per
    # twenty survivors, are projected at the measured night's count.
    h = HERMETIC_SHARDS if p["mode"] == "complete" or not rec["hermetic_shards"] else rec["hermetic_shards"]
    hermetic = (math.ceil(h / st["hermetic_parallel"]) * (rec["hermetic"] or 0)) if n else 0
    stage_m = max(fuzz, hermetic) + REACH_MINUTES
    end = now + datetime.timedelta(minutes=matrix + stage_m)
    out.update(mutants_per_shard=round(per_shard, 1), shard_minutes=round(shard, 1), waves=waves_n,
               hermetic_shards=h if n else 0,
               matrix_minutes=round(matrix), stage_minutes=round(stage_m),
               end=end.strftime("%Y-%m-%dT%H:%M:%SZ"))
    return out


def projection_line(pr):
    if pr["end"] is None:
        return (f"The projection at cap {pr['cap']}: {pr['shards']} shards, no measured shard time in the record; "
                f"no end projected.")
    m, s = divmod(pr["matrix_minutes"] + pr["stage_minutes"], 60)
    return (f"The projection at cap {pr['cap']}: {pr['shards']} shards, {pr['parallel']} at once, "
            f"{pr['mutants_per_shard']} mutants a shard at {pr['shard_minutes']} minutes "
            f"(the measured shard time of run {run_id(str(pr['measured_run']))}), {pr['waves']} wave(s), "
            f"the matrix {pr['matrix_minutes']} minutes, then the stage and reach {pr['stage_minutes']} minutes; "
            f"{m}h{s:02d}m from {pr['now'][11:16]}Z, the projected end {pr['end'][11:16]}Z "
            f"({pr['end'][:10]}); GitHub's own waits not projected, GitHub's.")


def capped(args, mode, record, paths, root, head):
    """The plan at the dispatch's cap (#584), its projection, and the window's word:
    fourteen whose projected end falls past the window's end, or that the record
    cannot project, falls back to six, saying so."""
    if args.cap not in CAPS:
        fault(f"cap {args.cap} is not one of {', '.join(map(str, CAPS))} (#584)")
    now = (datetime.datetime.fromisoformat(args.now.replace("Z", "+00:00")) if args.now
           else datetime.datetime.now(datetime.timezone.utc))
    end = window_end_after(now, args.window_end) if args.window_end else None
    rec = measured(record)
    p = plan(mode, record, paths, root, head, args.max_shards, args.complete_shards, args.cap)
    p["projection"] = project(p, rec, now, args.fuzz_seconds)
    p["requested_cap"], p["window_end"], p["cap_why"] = args.cap, args.window_end or None, None
    if end is None:
        p["cap_why"] = f"cap {args.cap} as dispatched; no window named, so no window check"
    elif p["projection"]["end"] is None or p["projection"]["end"] > end.strftime("%Y-%m-%dT%H:%M:%SZ"):
        if args.cap != HEAVY_RUN_PARALLEL:
            said = ("the record carries no measured shard time to project it" if p["projection"]["end"] is None
                    else f"its projected end {p['projection']['end'][11:16]}Z falls past the window's end {args.window_end}")
            refused = p["projection"]
            p = plan(mode, record, paths, root, head, args.max_shards, args.complete_shards, HEAVY_RUN_PARALLEL)
            p["projection"] = project(p, rec, now, args.fuzz_seconds)
            p["refused"] = refused
            p["requested_cap"], p["window_end"] = args.cap, args.window_end
            p["cap_why"] = (f"cap {args.cap} refused: {said} (#584); the night falls back to six, "
                            f"the table's word for a run that would overrun its window")
        else:
            p["cap_why"] = (f"cap 6; the projected end falls past the window's end {args.window_end} "
                            f"or is not projected: six is the table's floor, so it runs")
    else:
        p["cap_why"] = f"cap {args.cap}: the projected end falls inside the window, ending {args.window_end}"
    return p


def brings(files, floored):
    return "every floored file" if files == floored else ", ".join(f"`{f}`" for f in files)


def waves(p):
    n, k = p["shards"], p["parallel"]
    full, rest = divmod(n, k)
    sizes = [k] * full + ([rest] if rest else [])
    return f"{len(sizes)} wave{'s' if len(sizes) != 1 else ''}: {', then '.join(map(str, sizes))}"


def summary(p):
    lines = [f"## The night's plan ({p['mode']})", ""]
    lines.append(f"The base: {p['base']['run']} at `{p['base']['commit']}`, {p['base_why']}; the night's tree: `{p['head']}`.")
    lines.append("")
    if p["mode"] == "complete":
        lines.append(f"A complete night: every floored file, {p['mutants']} mutants by the record, in {p['shards']} shards, "
                     f"{p['parallel']} at once ({waves(p)}).")
    elif not p["files"]:
        lines.append("No floored file changed and no test or dependency reaches one: the night mutates nothing "
                     "and carries every file's figure from the base.")
    else:
        lines.append(f"Mutated: {', '.join(f'`{f}`' for f in p['files'])} ({p['mutants']} mutants by the record, "
                     f"{p['shards']} shards, {p['parallel']} at once){'; the whole set' if p['whole'] else ''}.")
        if p["whole"]:
            lines.append("The changed set is the whole set: the night's record re-bases on this run "
                         "(base: this run, the changed set the whole set; #559).")
        lines.append(f"Carried from the base: {', '.join(f'`{f}`' for f in p['carried']) or 'none'}.")
    st = p["stage"]
    lines.append("")
    lines.append(f"After the matrix (#517): the fuzz targets {st['fuzz_parallel']} at once beside the hermetic "
                 f"shards {st['hermetic_parallel']} at a time ({st['fuzz_parallel'] + st['hermetic_parallel']} "
                 f"of the cap's {st['cap']}), then the reach job and the bench (on a complete night or with `bench`), "
                 f"{st['tail']} at most, last.")
    if "projection" in p:
        lines += ["", f"The cap (#584): {p['cap']} at once; {p['cap_why']}."]
        if "refused" in p:
            lines.append(projection_line(p["refused"]) + " Refused.")
        lines.append(projection_line(p["projection"]))
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
    ap.add_argument("--cap", type=int, default=HEAVY_RUN_PARALLEL, help="the dispatch's cap, 6 or 14 (#584)")
    ap.add_argument("--window-end", default="", help="the window's end, HH:MMZ; empty for no window check")
    ap.add_argument("--now", default=None, help="the plan's minute, ISO (else the clock)")
    ap.add_argument("--fuzz-seconds", type=int, default=600, help="the night's fuzz budget a target")
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
    p = capped(args, args.mode, record, paths, root, head)
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
            out.write(f"cap={p['cap']}\n")
            out.write(f"fuzz_parallel={p['stage']['fuzz_parallel']}\n")
            out.write(f"hermetic_parallel={p['stage']['hermetic_parallel']}\n")
            out.write(f"empty={'true' if not p['files'] else 'false'}\n")
            out.write(f"selection={p['selection']}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
