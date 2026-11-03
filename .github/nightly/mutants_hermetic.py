#!/usr/bin/env python3
"""The Nightly's hermetic pass over the cargo pass's survivors (#393).

Usage:
  mutants_hermetic.py survivors <cargo outcomes dir> <out file>
  mutants_hermetic.py shard <survivors file> <shard> <shards> <names out> <regexes out>
  mutants_hermetic.py check-list <names file> <list file>

`survivors` reads every `outcomes.json` under the directory (the plan's cargo
shards') and writes each mutant name whose outcome is missed or timeout, once,
sorted: the mutants no cargo test target caught. A caught mutant is caught (a
second judgment adds nothing) and an unviable one does not build, so neither
is a survivor. Each name is written once; a night that judged a name twice
is refused by the score (#400).

`shard` deals the survivors round-robin (name i to shard i mod shards) and
writes the shard's names and, one a line, an anchored regex matching exactly
each name (`^name$`, every character Rust's `regex::escape` escapes escaped),
for cargo-mutants' `-F`.

`check-list` compares the names cargo-mutants' `--list` printed under those
regexes with the shard's names, as sets: exit 1, naming each difference, when
they are not equal, so a shard never judges a selection other than its own;
and exit 1 naming a name the list printed twice (#400: a mutant is judged
once). Both sides are compared colour-free (#435): an ANSI SGR escape
(`ESC[...m`) is stripped from every line first, since the job's
`CARGO_TERM_COLOR=always` once coloured the listing and refused a right
selection; the list step asks for none (`--colors never`, `NO_COLOR=1`), and
the strip is the defence. A selection other than the survivors is still
refused.
"""

import json
import re
import sys
from collections import Counter
from pathlib import Path

SURVIVING = ("MissedMutant", "Timeout")
# regex-syntax's is_meta_character: what `regex::escape` escapes.
META = set("\\.+*?()|[]{}^$#&-~")
# An ANSI SGR escape, the colour cargo-mutants prints (#435).
SGR = re.compile(r"\x1b\[[0-9;]*m")


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


def plain(line):
    return SGR.sub("", line)


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
        listed = [n for n in map(plain, read_names(args[2])) if n.strip()]
        want = {n for n in map(plain, read_names(args[1])) if n.strip()}
        got = set(listed)
        twice = sorted(n for n, k in Counter(listed).items() if k > 1)
        for n in twice:
            print(f"listed more than once: {n}")
        for n in sorted(want - got):
            print(f"not selected: {n}")
        for n in sorted(got - want):
            print(f"selected, not a survivor of this shard: {n}")
        if want != got or twice:
            print(f"the selection is not the shard's survivors ({len(listed)} listed, {len(got)} distinct, {len(want)} named)")
            return 1
        print(f"the selection is the shard's {len(want)} survivors, name for name")
        return 0
    print(__doc__, file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
