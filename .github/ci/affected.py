#!/usr/bin/env python3
"""The tests a pull request runs: the targets its diff reaches (#473).

Usage:
  affected.py --event <github.event_name> --paths <file>
              [--github-output <file>] [--root <dir>]

CI's Test and wire jobs run, on `pull_request`, the test targets the PR's
diff reaches, and on every other event (`merge_group`, a push to beta or
main, a tag's `workflow_call`) the whole suite: a PR's green is the reached
tests', the queue's green is the suite's (docs/lanes/README.md rule 17).
The oracle is the incremental Nightly's (.github/nightly/changed_set.py,
#462): the crate's module tree read from each crate root's `mod`
declarations and the reach record of #401
(.github/nightly/no-floored-reach.json: each target it measures, with the
lines of each floored file it ran under cargo llvm-cov). `--paths` holds the
diff's changed paths less its docs (the `changes` job's prose, rule 16),
one a line. A path's effect:

- the whole suite: a lockfile or a manifest (`Cargo.lock`, `Cargo.toml`,
  anywhere), the planner's whole-set files (the toolchain, the cargo config,
  `build.rs`), anything under `.github/` or `tools/`, anything under `mcp/`
  (no record measures the companion's tests), and every path the rules below
  do not place: a file with no reach record runs everything;
- a Rust file under `tests/` or `fuzz/`: the targets that compile it. A
  target compiles its own file (`tests/<name>.rs`, or `tests/<name>/` with
  its `main.rs`) and the modules it declares (`mod`, at their `#[path]`,
  followed through each file); so `tests/wire_support/` runs `wire` and
  `wire_consumers`, `fuzz/src/` runs `fuzz_replay`. Any other file there (a
  fixture, a vector, a corpus, the e2e harness: a target may read it at run
  time, even one that also includes it by `include_str!`), and a Rust file
  no target compiles, runs everything;
- a bench (`benches/`): no test (cargo test builds no bench; Clippy's
  `--all-targets` does);
- a file under `src/`:
  - always the targets that read the source tree as text (a target whose
    files hold the string `"src"` or `"src/...`: the architecture scanners,
    rule 28's sites, the hermetic suite's staleness check), since a line
    they read moves without a line run, and the targets that compile it by
    `#[path]`;
  - test-only code (declared under `#[cfg(test)]` in every crate that
    compiles it): the lib's and the binaries' unit tests (`--lib --bins`);
  - a floored file (the reach record's `floored`): the unit tests and every
    test target but those the record measures running no line of it;
  - a file a source file reads by `include_str!`/`include_bytes!`: its
    readers' effect;
  - any other file under `src/`: the whole suite (the record measures no
    target's reach into it).

"Its own unit tests" are the lib's and the binaries': `--lib --bins` runs
every unit test of the workspace, a superset, so nextest's list stays
cargo's for the same selection.

The output (`--github-output`): `mode` (`all` or `affected`), `test_args`
(cargo's target selection for Test, `--workspace` alone when all), `test_run`
(false when no test target is reached), `wire` (whether the wire target is
reached), `deny` (whether a path cargo-deny reads changed: the manifests,
the lockfile, `deny.toml`, `template/stack.toml`, ci.yml; always true off a
PR) and `line` (the selection, one line, for the log). An empty output, as a
failed `changes` job leaves, means the whole suite to every reader.
"""

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "nightly"))
import changed_set  # noqa: E402  (the Nightly's oracle: the module tree, the reach record)
import no_floored  # noqa: E402  (the one reader of the floors)

ROOT = HERE.parent.parent
# cargo-deny reads the workspace's manifests and lock, its config and the
# template's stamp; ci.yml holds its steps.
DENY = {"Cargo.toml", "Cargo.lock", "mcp/Cargo.toml", "deny.toml", "template/stack.toml",
        ".github/workflows/ci.yml"}
MOD = re.compile(r"((?:#\[[^\]]*\]\s*)*)(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;")
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
READS_SRC = re.compile(r'"src(?:/[^"]*)?"')
COMMENT = re.compile(r"//[^\n]*")


def code(text):
    """Rust source without its line comments (a doc comment naming src/ is not a read)."""
    return COMMENT.sub("", text)


def rel(root, p):
    return p.resolve().relative_to(root.resolve()).as_posix()


def compiled(root, top):
    """The Rust files a test target compiles, from its top file: its `mod`s
    at their `#[path]` (a file a `#[path]` loads, a `mod.rs` or a `main.rs`
    holds its children beside it, any other file in its stem's directory;
    both are tried), followed through each file."""
    seen, todo = set(), [(top, True)]
    while todo:
        f, dir_owner = todo.pop()
        r = rel(root, f)
        if r in seen:
            continue
        seen.add(r)
        text = code(f.read_text())
        homes = [f.parent] if dir_owner or f.name in ("mod.rs", "main.rs") else [f.parent, f.with_suffix("")]
        for attrs, name in MOD.findall(text):
            kept = PATH_ATTR.search(attrs)
            if kept:
                cands = [(f.parent / kept.group(1), True)]
            else:
                cands = [(h / f"{name}.rs", False) for h in homes] + [(h / name / "mod.rs", True) for h in homes]
            for c, owner in cands:
                if c.is_file():
                    todo.append((c, owner))
    return seen


def test_targets(root):
    """{(kind, name): the files it compiles}, for every target under tests/."""
    out = {}
    for kind, name in changed_set.targets(root):
        if kind != "test":
            continue
        top = root / "tests" / f"{name}.rs"
        if not top.is_file():
            top = root / "tests" / name / "main.rs"
        out[(kind, name)] = compiled(root, top)
    return out


def reads_src(root, files):
    return any(READS_SRC.search(code((root / f).read_text())) for f in files if f.endswith(".rs"))


class Oracle:
    def __init__(self, root):
        self.root = root
        self.targets = test_targets(root)
        self.text_readers = {t for t, files in self.targets.items() if reads_src(root, files)}
        self.floored = set(no_floored.floored(root))
        record = changed_set.reach_map(root, sorted(self.floored))
        # The record's own floored files: a file it does not name has no reach record.
        reach = root / changed_set.REACH
        named = set(json.loads(reach.read_text()).get("floored", [])) if reach.exists() else set()
        self.measured = {f: {t for t, hit in record.items() if t[0] == "test" and f not in hit}
                         for f in self.floored & named}
        self._crates = None

    @property
    def crates(self):
        if self._crates is None:
            self._crates = changed_set.Crates(self.root)
        return self._crates

    def compilers(self, path):
        return {t for t, files in self.targets.items() if path in files}

    def effect(self, path):
        """(whole, unit tests, targets, why)."""
        name = path.rsplit("/", 1)[-1]
        if name in ("Cargo.toml", "Cargo.lock"):
            return True, False, set(), "a manifest or a lockfile: a dependency counts as changing every target"
        if path in changed_set.WHOLE:
            return True, False, set(), changed_set.WHOLE[path]
        if path.startswith(".github/") or path.startswith("tools/"):
            return True, False, set(), "CI's or the tools' own shape"
        if path.startswith("mcp/"):
            return True, False, set(), "the MCP companion: no record measures its tests' reach"
        if path.startswith("benches/"):
            return False, False, set(), "a bench: cargo test builds none (Clippy --all-targets does)"
        if path.startswith("tests/") or path.startswith("fuzz/"):
            by = self.compilers(path) if path.endswith(".rs") else set()
            if by:
                return False, False, by, "compiled by " + ", ".join(sorted(n for _, n in by))
            return True, False, set(), ("not Rust a target compiles (a fixture, a vector, a corpus, a harness: "
                                        "any target may read it at run time): no reach record")
        if path.startswith("src/"):
            return self.source(path)
        return True, False, set(), "no reach record"

    def source(self, path, depth=0):
        extra = self.text_readers | self.compilers(path)
        crates = self.crates
        if path in self.measured:
            skip = self.measured[path] - extra
            hit = set(self.targets) - skip
            return False, True, hit, ("a floored file: the unit tests and every target but "
                                      + (", ".join(sorted(n for _, n in skip)) or "none")
                                      + " (the reach record, #401: no line of it run)")
        if path in crates.places:
            if crates.is_test_only(path):
                return False, True, extra, "test-only code: the unit tests, and the targets that read src/ as text"
            return True, False, set(), "code the lib or a binary compiles: the record measures no target's reach into it"
        readers = crates.includes().get(path)
        if readers and depth < 4:
            whole, unit, hit = False, False, set(extra)
            for r in sorted(readers):
                w, u, h, _ = self.source(r, depth + 1)
                whole, unit, hit = whole or w, unit or u, hit | h
            return whole, unit, hit, "read by " + ", ".join(sorted(readers)) + " (include_str!/include_bytes!)"
        return True, False, set(), "a file under src/ no module tree holds: no reach record"


def select(event, paths, root):
    """The selection: a dict of the outputs and each path's effect."""
    if event != "pull_request":
        return {"mode": "all", "unit": True, "targets": [], "deny": True, "paths": [],
                "why": f"{event}: the whole suite"}
    oracle = Oracle(root)
    whole, unit, hit, found = False, False, set(), []
    for p in paths:
        w, u, h, why = oracle.effect(p)
        whole, unit, hit = whole or w, unit or u, hit | h
        found.append({"path": p, "whole": w, "unit": u, "targets": sorted(n for _, n in h), "why": why})
    deny = any(p in DENY for p in paths)
    if whole or not paths:
        return {"mode": "all", "unit": True, "targets": [], "deny": deny, "paths": found,
                "why": "a path with no narrower reach" if paths else "no changed path: the whole suite"}
    return {"mode": "affected", "unit": unit, "targets": sorted(n for _, n in hit), "deny": deny,
            "paths": found, "why": "the targets the diff reaches"}


def outputs(s):
    if s["mode"] == "all":
        args, run, wire = "--workspace", True, True
        line = f"selection: all (the whole suite; {s['why']})"
    else:
        words = ["--workspace"] + (["--lib", "--bins"] if s["unit"] else [])
        words += [f"--test {t}" for t in s["targets"]]
        args, run, wire = " ".join(words), bool(s["unit"] or s["targets"]), "wire" in s["targets"]
        reached = (["the unit tests"] if s["unit"] else []) + s["targets"]
        line = ("selection: affected (" + (", ".join(reached) if reached else "no test target") + ")")
    return {"mode": s["mode"], "test_args": args, "test_run": str(run).lower(), "wire": str(wire).lower(),
            "deny": str(s["deny"]).lower(), "line": line}


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--event", required=True)
    ap.add_argument("--paths", required=True, help="the changed paths less the docs, one a line")
    ap.add_argument("--github-output", default=None)
    ap.add_argument("--root", default=None)
    args = ap.parse_args(argv)
    root = Path(args.root).resolve() if args.root else ROOT
    paths = [l.strip() for l in Path(args.paths).read_text().splitlines() if l.strip()]
    s = select(args.event, paths, root)
    out = outputs(s)
    for e in s["paths"]:
        reach = "the whole suite" if e["whole"] else ", ".join((["unit tests"] if e["unit"] else []) + e["targets"]) or "no test"
        print(f"  {e['path']}: {reach} ({e['why']})")
    print(out["line"])
    print(f"cargo's selection: {out['test_args']}; wire: {out['wire']}; deny: {out['deny']}")
    if args.github_output:
        with open(args.github_output, "a") as f:
            for k, v in out.items():
                f.write(f"{k}={v}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
