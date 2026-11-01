#!/usr/bin/env python3
"""The Nightly's hermetic pass over the cargo pass's survivors (#393).

Usage:
  mutants_hermetic.py survivors <cargo outcomes dir> <out file>
  mutants_hermetic.py shard <survivors file> <shard> <shards> <names out> <regexes out>
  mutants_hermetic.py check-list <names file> <list file>

`survivors` reads every `outcomes.json` under the directory (the fifty cargo
shards') and writes each mutant name whose outcome is missed or timeout, once,
sorted: the mutants no cargo test target caught. A caught mutant is caught (a
second judgment adds nothing) and an unviable one does not build, so neither
is a survivor. A file a binary and the library both declare (`mod explorers;`
in main.rs and lib.rs) lists a mutant twice under one name; the name is
written once and cargo-mutants selects both copies by it.

`shard` deals the survivors round-robin (name i to shard i mod shards) and
writes the shard's names and, one a line, an anchored regex matching exactly
each name (`^name$`, every character Rust's `regex::escape` escapes escaped),
for cargo-mutants' `-F`.

`check-list` compares the names cargo-mutants' `--list` printed under those
regexes with the shard's names, as sets: exit 1, naming each difference, when
they are not equal, so a shard never judges a selection other than its own.
"""

import json
import sys
from pathlib import Path

SURVIVING = ("MissedMutant", "Timeout")
# regex-syntax's is_meta_character: what `regex::escape` escapes.
META = set("\\.+*?()|[]{}^$#&-~")


def mutant_outcomes(root):
    for path in sorted(Path(root).rglob("outcomes.json")):
        for outcome in json.loads(path.read_text())["outcomes"]:
            scenario = outcome["scenario"]
            if isinstance(scenario, dict) and "Mutant" in scenario:
                yield scenario["Mutant"], outcome["summary"]


def survivors(root):
    return sorted({m["name"] for m, summary in mutant_outcomes(root) if summary in SURVIVING})


def escape(name):
    return "".join("\\" + c if c in META else c for c in name)


def read_names(path):
    return [line for line in Path(path).read_text().splitlines() if line.strip()]


def write_lines(path, lines):
    Path(path).write_text("".join(f"{line}\n" for line in lines))


def main(args):
    if args[:1] == ["survivors"] and len(args) == 3:
        names = survivors(args[1])
        write_lines(args[2], names)
        print(f"{len(names)} survivors of the cargo pass (missed or timeout) in {args[2]}")
        return 0
    if args[:1] == ["shard"] and len(args) == 6:
        names, shard, shards = read_names(args[1]), int(args[2]), int(args[3])
        if not 0 <= shard < shards:
            print(f"shard {shard} is not in 0..{shards - 1}", file=sys.stderr)
            return 1
        mine = [n for i, n in enumerate(names) if i % shards == shard]
        write_lines(args[4], mine)
        write_lines(args[5], [f"^{escape(n)}$" for n in mine])
        print(f"shard {shard}/{shards}: {len(mine)} of {len(names)} survivors")
        return 0
    if args[:1] == ["check-list"] and len(args) == 3:
        want, got = set(read_names(args[1])), set(read_names(args[2]))
        for n in sorted(want - got):
            print(f"not selected: {n}")
        for n in sorted(got - want):
            print(f"selected, not a survivor of this shard: {n}")
        if want != got:
            print(f"the selection is not the shard's survivors ({len(got)} selected, {len(want)} named)")
            return 1
        print(f"the selection is the shard's {len(want)} survivors, name for name")
        return 0
    print(__doc__, file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
