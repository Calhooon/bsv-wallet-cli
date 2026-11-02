#!/usr/bin/env python3
"""The Nightly's plan: which floored files a night mutates (#462).

Usage:
  changed_set.py --mode incremental|complete [--record <mutants.json>]
                 [--head <sha>] [--paths <file>] [--out <plan.json>]
                 [--github-output <file>]

The Nightly has two modes. A COMPLETE night mutates every floored file (the
files of MUTANTS_FLOORS in .github/workflows/nightly.yml) in MUTANTS_SHARDS
shards, the night of record cut at each pin (docs/RELEASING.md). An
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
- anything else (another source file, the docs, the tools, `benches/baseline.json`,
  the fuzz crate's own manifest and lock): nothing.

The plan names the selection the shards pass `cargo test` (`--lib --bins`, a
`--test` for every target under tests/ and a `--bench` for every bench, less
the left-out list's entries: the oracle complete per floored file, the owner's
23.9 word), the shard count (a complete night or a whole set: MUTANTS_SHARDS;
otherwise one shard per PER_SHARD of the changed files' mutants, by the
record's counts, and at least PARALLEL (the six a night runs at once) while
there are that many mutants, at most MUTANTS_SHARDS) and the matrix. An
empty changed set plans no shard: the night carries every figure and says so.

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
# The complete night's sizing: 923 mutants in fifty shards, 19 at most a shard.
PER_SHARD = 19
# The matrix's max-parallel (#144): a small set still fills the six runners a
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


def effects(paths, floored, reach, root):
    """Each changed path's effect: [(path, files it brings in, why)]."""
    out = []
    every = set(floored)
    for path in paths:
        if path in floored:
            out.append((path, {path}, "a floored file"))
        elif path in WHOLE:
            out.append((path, every, WHOLE[path]))
        elif is_test_input(path):
            target = target_of(path, root)
            if target is None:
                out.append((path, every, "a shared input of the test targets: no record names its reach per file"))
            elif target in reach:
                hit = reach[target]
                why = (f"{target[0]} {target[1]}: it reaches "
                       + (", ".join(sorted(hit)) if hit else "no floored file (the reach record, #401)"))
                out.append((path, hit, why))
            else:
                out.append((path, every, f"{target[0]} {target[1]}: the reach record does not measure it"))
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


def plan(mode, record, paths, root, head, max_shards):
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
    elif mode == "complete" or whole:
        shards = max_shards
    else:
        shards = min(max_shards, max(1, math.ceil(mutants / PER_SHARD), min(PARALLEL, mutants)))
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
        "selection": selection(root),
        "paths": [{"path": p, "files": sorted(f), "why": why} for p, f, why in found],
    }


def brings(files, floored):
    return "every floored file" if files == floored else ", ".join(f"`{f}`" for f in files)


def summary(p):
    lines = [f"## The night's plan ({p['mode']})", ""]
    lines.append(f"The last complete run: {p['base']['run']} at `{p['base']['commit']}`; the night's tree: `{p['head']}`.")
    lines.append("")
    if p["mode"] == "complete":
        lines.append(f"A complete night: every floored file, {p['mutants']} mutants by the record, in {p['shards']} shards.")
    elif not p["files"]:
        lines.append("No floored file changed and no test or dependency reaches one: the night mutates nothing "
                     "and carries every file's figure from the last complete run.")
    else:
        lines.append(f"Mutated: {', '.join(f'`{f}`' for f in p['files'])} ({p['mutants']} mutants by the record, "
                     f"{p['shards']} shards){'; the whole set' if p['whole'] else ''}.")
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
    ap.add_argument("--max-shards", type=int, default=50)
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
    p = plan(args.mode, record, paths, root, head, args.max_shards)
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
            out.write(f"empty={'true' if not p['files'] else 'false'}\n")
            out.write(f"selection={p['selection']}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
