#!/usr/bin/env python3
"""The guard on the Mutants shards' left-out targets (#401).

Usage:
  no_floored.py list test|bench
  no_floored.py measure <record out> [--lcov-dir <dir>]
  no_floored.py check <record>

The Nightly's cargo oracle is every test target and bench that runs a line
of a floored file (#310); the ones that run none are left out, by name, in
.github/nightly/no-floored-targets. That list is the one place the names
live: the selection step reads it with `list`, and the guard reads it here.

`list` prints the names of one kind, space-separated, for the selection.

`measure` runs `cargo llvm-cov --locked --lcov --test <name>` (or
`--bench <name>`) for each entry, which also counts the binary a target
starts, and writes the record: each entry with the lines of each floored file
it ran (the files of MUTANTS_FLOORS in .github/workflows/nightly.yml), the
commit and the seconds each measurement took. It then checks the record as
`check` does and exits as `check` would.

`check` reads a record and fails (exit 1), naming each fault, when the record
and the tree disagree: an entry of the list the record does not measure, or
a record entry the list does not hold; an entry that names no target; a
floored file the record does not measure; and above all an entry that runs a
line of a floored file, named with its kind, its name and the first such
line (`test arch_no_unwrap runs src/spend_gate.rs:120`): that target judges a
floored file, so it joins the oracle and leaves the list. CI's ratchet job
runs `check` on the committed record, the Nightly runs `measure` on the
night's tree.
"""

import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
LIST = ".github/nightly/no-floored-targets"
NIGHTLY = ".github/workflows/nightly.yml"
KINDS = ("test", "bench")


def entries(root):
    """The list's `(kind, name)` pairs, in order; a malformed line or a name
    listed twice is a fault."""
    out, faults = [], []
    for n, line in enumerate((root / LIST).read_text().splitlines(), 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        words = line.split()
        if len(words) != 2 or words[0] not in KINDS:
            faults.append(f"{LIST}:{n}: not `test <name>` or `bench <name>`: {line!r}")
            continue
        if tuple(words) in out:
            faults.append(f"{LIST}:{n}: {words[0]} {words[1]} is listed twice")
            continue
        out.append(tuple(words))
    return out, faults


def floored(root):
    """The floored files: the keys of MUTANTS_FLOORS's folded JSON object."""
    lines = (root / NIGHTLY).read_text().splitlines()
    found = [i for i, line in enumerate(lines) if re.match(r"\s*MUTANTS_FLOORS:", line)]
    if len(found) != 1:
        sys.exit(f"{NIGHTLY}: MUTANTS_FLOORS must be set once, is set {len(found)} times")
    i = found[0]
    m = re.match(r"(\s*)MUTANTS_FLOORS:\s*(.*)$", lines[i])
    indent, rest = len(m.group(1)), m.group(2).strip()
    if rest and rest not in (">", ">-", "|", "|-"):
        text = rest.strip("'\"")
    else:
        body = []
        for nxt in lines[i + 1:]:
            if nxt.strip() and len(nxt) - len(nxt.lstrip()) <= indent:
                break
            body.append(nxt.strip())
        text = " ".join(body)
    value = json.loads(text)
    if not isinstance(value, dict) or not value:
        sys.exit(f"{NIGHTLY}: MUTANTS_FLOORS holds no floor")
    return sorted(value)


def target_exists(root, kind, name):
    if kind == "test":
        return (root / "tests" / f"{name}.rs").is_file() or (root / "tests" / name / "main.rs").is_file()
    return (root / "benches" / f"{name}.rs").is_file()


def lcov_reach(text, root, files):
    """Each floored file's lines with a nonzero count in an lcov report, and
    the floored files the report names (a file it never names is unread,
    never zero)."""
    reach = {f: set() for f in files}
    seen = set()
    current = None
    for line in text.splitlines():
        if line.startswith("SF:"):
            path = Path(line[3:])
            try:
                rel = path.resolve().relative_to(root.resolve()).as_posix()
            except ValueError:
                rel = path.as_posix()
            current = rel if rel in reach else None
            if current:
                seen.add(current)
        elif line.startswith("DA:") and current:
            number, count = line[3:].split(",")[:2]
            if int(count) > 0:
                reach[current].add(int(number))
        elif line == "end_of_record":
            current = None
    return {f: sorted(lines) for f, lines in reach.items()}, seen


def check(record, root):
    """The faults of a record against the tree, as lines; none when it holds."""
    listed, faults = entries(root)
    files = floored(root)
    measured = {(t["kind"], t["name"]): t for t in record.get("targets", [])}
    for kind, name in listed:
        if not target_exists(root, kind, name):
            faults.append(f"{kind} {name} is listed in {LIST} but is no target of the package")
        if (kind, name) not in measured:
            faults.append(f"{kind} {name} is listed in {LIST} but the record does not measure it; run `{Path(__file__).name} measure`")
    for kind, name in measured:
        if (kind, name) not in listed:
            faults.append(f"{kind} {name} is in the record but not in {LIST}; re-measure the list")
    for f in files:
        if f not in record.get("floored", []):
            faults.append(f"the record does not measure the floored file {f}; run `{Path(__file__).name} measure`")
    for (kind, name), target in measured.items():
        for f in files:
            lines = target.get("reach", {}).get(f)
            if lines is None:
                if f in record.get("floored", []):
                    faults.append(f"{kind} {name}: the record holds no reach of {f}")
                continue
            if lines:
                more = f" and {len(lines) - 1} more of its lines" if len(lines) > 1 else ""
                faults.append(
                    f"{kind} {name} runs {f}:{lines[0]}{more}: it judges a floored file, "
                    f"so it joins the oracle; take it out of {LIST}")
    return faults


def report(faults, what, out=sys.stdout):
    for fault in faults:
        print(f"::error::{fault}", file=out)
    if faults:
        print(f"{what}: {len(faults)} fault(s); the left-out targets' zero reach does not hold", file=out)
        return 1
    print(f"{what}: every left-out target runs no line of a floored file", file=out)
    return 0


def measure(out, lcov_dir, root):
    listed, faults = entries(root)
    if faults:
        return report(faults, LIST)
    files = floored(root)
    lcov_dir = Path(lcov_dir or tempfile.mkdtemp(prefix="no-floored-"))
    lcov_dir.mkdir(parents=True, exist_ok=True)
    targets = []
    for kind, name in listed:
        path = lcov_dir / f"{kind}-{name}.lcov"
        cmd = ["cargo", "llvm-cov", "--locked", "--lcov", "--output-path", str(path), f"--{kind}", name]
        print("+ " + " ".join(cmd), flush=True)
        start = time.monotonic()
        done = subprocess.run(cmd, cwd=root)
        seconds = round(time.monotonic() - start, 1)
        if done.returncode != 0:
            print(f"::error::{kind} {name}: cargo llvm-cov exited {done.returncode}; its reach is not measured")
            return 1
        reach, seen = lcov_reach(path.read_text(), root, files)
        unread = [f for f in files if f not in seen]
        if unread:
            print(f"::error::{kind} {name}: the coverage report names no {', '.join(unread)}; its reach is unread, never zero")
            return 1
        print(f"{kind} {name}: {seconds} s; lines of the floored files: "
              + ", ".join(f"{f} {len(v)}" for f, v in reach.items()), flush=True)
        targets.append({"kind": kind, "name": name, "seconds": seconds, "reach": reach})
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True).stdout.strip()
    record = {
        "issue": 401,
        "commit": commit,
        "method": "cargo llvm-cov --locked --lcov --test|--bench <name>, one run a target",
        "floored": files,
        "targets": targets,
    }
    Path(out).write_text(json.dumps(record, indent=2) + "\n")
    print(f"wrote {out}; {sum(t['seconds'] for t in targets):.1f} s of measurement")
    return report(check(record, root), "measure")


def main(argv):
    root = ROOT
    if len(argv) == 2 and argv[0] == "list" and argv[1] in KINDS:
        listed, faults = entries(root)
        if faults:
            # stderr: the selection reads stdout into a variable.
            return report(faults, LIST, sys.stderr)
        print(" ".join(name for kind, name in listed if kind == argv[1]))
        return 0
    if len(argv) in (2, 4) and argv[0] == "measure":
        lcov_dir = None
        if len(argv) == 4:
            if argv[2] != "--lcov-dir":
                sys.exit(__doc__)
            lcov_dir = argv[3]
        return measure(argv[1], lcov_dir, root)
    if len(argv) == 2 and argv[0] == "check":
        return report(check(json.loads(Path(argv[1]).read_text()), root), "check")
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
