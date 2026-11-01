#!/usr/bin/env python3
"""The mutation score per file from cargo-mutants' outcomes (#75).

Usage: mutants_score.py [--hermetic <dir>] <outcomes.json or a directory holding them>...

Reads every `outcomes.json` given (one per shard), and prints a Markdown
table of each file's score: caught / (caught + missed + timeout) * 100. A
timeout is its own column and is never caught: a mutant whose tests ran
past the limit was not shown wrong by a test (it may be a loop the mutant
made, or a slow test), so it weighs against the score as a miss does. An
unviable mutant (one that does not build) counts nowhere. The floors are
`MUTANTS_FLOORS`, a JSON object of file to percent; a file the object does
not name has the floor 80. The table goes to `$GITHUB_STEP_SUMMARY` when it
is set. Exit 1 when a floored file scores below its floor or when a floored
file has no outcome at all (a shard that did not report is not a pass).

With `--hermetic <dir>` (#393) the score is the union of the two passes: the
outcomes given are the cargo pass's, and the directory holds the hermetic
pass's (`tests/e2e_hermetic.rs` under `E2E_HERMETIC_ORACLE=1`, which judged
only the cargo pass's survivors, by name). A survivor the hermetic pass caught
is caught, and is counted in the column `by e2e_hermetic` too; any other
outcome stands as the cargo pass gave it. Exit 1 when a survivor has no
hermetic outcome: a survivor the hermetic pass did not judge is no union.
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


def mutant_outcomes(args):
    for path in outcome_files(args):
        for outcome in json.loads(path.read_text())["outcomes"]:
            scenario = outcome["scenario"]
            if isinstance(scenario, dict) and "Mutant" in scenario:
                yield scenario["Mutant"], outcome["summary"]


def main(args):
    hermetic = None
    if args[:1] == ["--hermetic"]:
        if len(args) < 2:
            print("--hermetic needs a directory", file=sys.stderr)
            return 1
        # Each name the hermetic pass judged: caught when any copy was.
        hermetic = defaultdict(bool)
        for mutant, summary in mutant_outcomes([args[1]]):
            hermetic[mutant["name"]] |= summary == "CaughtMutant"
        args = args[2:]
    floors = {f: float(v) for f, v in json.loads(os.environ.get("MUTANTS_FLOORS", "{}")).items()}
    counts = defaultdict(lambda: defaultdict(int))
    by_hermetic = defaultdict(int)
    missed = defaultdict(list)
    unjudged = set()
    for mutant, summary in mutant_outcomes(args):
        if hermetic is not None and summary in ("MissedMutant", "Timeout"):
            if mutant["name"] not in hermetic:
                unjudged.add(mutant["name"])
            elif hermetic[mutant["name"]]:
                summary = "CaughtMutant"
                by_hermetic[mutant["file"]] += 1
        counts[mutant["file"]][summary] += 1
        if summary == "MissedMutant":
            missed[mutant["file"]].append(mutant["name"])

    lines = ["## Mutation score per file", ""]
    if hermetic is not None:
        lines += [
            "The union of two passes (#393): the cargo targets, then `e2e_hermetic` (the hermetic",
            "suite under `serve` without a tracker pass, the classic ARC stub, the explorers walled)",
            "on the cargo pass's survivors; `caught` counts both, `by e2e_hermetic` the second's.",
            "",
        ]
    lines += [
        "| file | caught | timeout | missed | unviable | score | floor | |"
        + (" by e2e_hermetic |" if hermetic is not None else ""),
        "|---|---:|---:|---:|---:|---:|---:|---|" + ("---:|" if hermetic is not None else ""),
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
            value = 100.0 * caught / judged
            score = f"{value:.1f}"
            mark = "below the floor" if value < floor else "ok"
            failed |= value < floor
        lines.append(
            f"| `{file}` | {caught} | {timeout} | {miss} | {unviable} | {score} | {floor:g} | {mark} |"
            + (f" {by_hermetic.get(file, 0)} |" if hermetic is not None else "")
        )
    # A file a binary and the library both declare as a module (`mod
    # explorers;` in main.rs and lib.rs) is mutated once per crate: the
    # counts above weigh both copies alike, the list names each once.
    for file in sorted(missed):
        names = sorted(set(missed[file]))
        lines += ["", f"<details><summary>{len(names)} missed in <code>{file}</code></summary>", ""]
        lines += [f"- `{name}`" for name in names]
        lines += ["", "</details>"]
    if unjudged:
        failed = True
        lines += ["", f"**{len(unjudged)} survivor(s) of the cargo pass have no hermetic outcome**; no union:", ""]
        lines += [f"- `{name}`" for name in sorted(unjudged)]
    text = "\n".join(lines) + "\n"
    sys.stdout.write(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write(text)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
