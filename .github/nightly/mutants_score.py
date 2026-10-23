#!/usr/bin/env python3
"""The mutation score per file from cargo-mutants' outcomes (#75).

Usage: mutants_score.py <outcomes.json or a directory holding them>...

Reads every `outcomes.json` given (one per shard), and prints a Markdown
table of each file's score: caught / (caught + missed + timeout) * 100, a
timeout counting as caught (the tests did not pass on the mutant) and an
unviable mutant (one that does not build) counting nowhere. The floors are
`MUTANTS_FLOORS`, a JSON object of file to percent; a file the object does
not name has the floor 80. The table goes to `$GITHUB_STEP_SUMMARY` when it
is set. Exit 1 when a floored file scores below its floor or when a floored
file has no outcome at all (a shard that did not report is not a pass).
"""

import json
import os
import sys
from collections import defaultdict
from pathlib import Path

DEFAULT_FLOOR = 80.0


def outcome_files(args):
    for arg in args:
        path = Path(arg)
        if path.is_dir():
            yield from sorted(path.rglob("outcomes.json"))
        else:
            yield path


def main(args):
    floors = {f: float(v) for f, v in json.loads(os.environ.get("MUTANTS_FLOORS", "{}")).items()}
    counts = defaultdict(lambda: defaultdict(int))
    missed = defaultdict(list)
    for path in outcome_files(args):
        for outcome in json.loads(path.read_text())["outcomes"]:
            scenario = outcome["scenario"]
            if not isinstance(scenario, dict) or "Mutant" not in scenario:
                continue
            mutant = scenario["Mutant"]
            counts[mutant["file"]][outcome["summary"]] += 1
            if outcome["summary"] == "MissedMutant":
                missed[mutant["file"]].append(mutant["name"])

    lines = [
        "## Mutation score per file",
        "",
        "| file | caught | timeout | missed | unviable | score | floor | |",
        "|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    failed = False
    for file in sorted(set(counts) | set(floors)):
        c = counts.get(file, {})
        caught, timeout = c.get("CaughtMutant", 0), c.get("Timeout", 0)
        miss, unviable = c.get("MissedMutant", 0), c.get("Unviable", 0)
        judged = caught + timeout + miss
        floor = floors.get(file, DEFAULT_FLOOR)
        if judged == 0:
            score, mark = "none", "no outcome"
            failed = True
        else:
            value = 100.0 * (caught + timeout) / judged
            score = f"{value:.1f}"
            mark = "below the floor" if value < floor else "ok"
            failed |= value < floor
        lines.append(
            f"| `{file}` | {caught} | {timeout} | {miss} | {unviable} | {score} | {floor:g} | {mark} |"
        )
    # A file a binary and the library both declare as a module (`mod
    # explorers;` in main.rs and lib.rs) is mutated once per crate: the
    # counts above weigh both copies alike, the list names each once.
    for file in sorted(missed):
        names = sorted(set(missed[file]))
        lines += ["", f"<details><summary>{len(names)} missed in <code>{file}</code></summary>", ""]
        lines += [f"- `{name}`" for name in names]
        lines += ["", "</details>"]
    text = "\n".join(lines) + "\n"
    sys.stdout.write(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write(text)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
