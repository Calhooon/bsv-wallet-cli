#!/usr/bin/env python3
"""The mutation score per file from cargo-mutants' outcomes (#75).

Usage: mutants_score.py [--sha <commit> [--survivors <dir>]] [--hermetic <dir>]
                        <outcomes.json or a directory holding them>...
       mutants_score.py --shards-only --plan <plan.json> <directory>...

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

With `--sha <commit>` (#423) a night is one tree: each shard's artifact (an
immediate subdirectory of each directory given, the cargo pass's and the
hermetic pass's, as `download-artifact` lays them out) holds a file `sha`, the
commit its job checked out, and `--survivors <dir>` names the survivors'
artifact, which holds one too. Before any figure is judged, exit 1 naming each
shard whose sha is missing or is not `<commit>` (the dispatch's), with the sha
it names; otherwise the night's commit is printed above the table.

A mutant is judged once (#400): a name with more than one outcome in the
cargo pass, or in the hermetic pass, is a malformed night (cargo-mutants
listed the mutant twice, or a shard's artifact was given twice), and before
any figure is judged the score exits 1 naming each such name and its count.
The counts per file are then the unique mutants'.

With `--plan <plan.json> --record <mutants.json>` (#462) the night is one of
the Nightly's two modes, as .github/nightly/changed_set.py planned it. The
mutated files (the plan's `files`) are scored from the outcomes; every other
floored file's figure is CARRIED from the record, which holds the last
complete run, and its row names that run and sha. A carried figure is never
judged against its floor again (its run judged it); the night's files are.
Before any figure is judged, exit 1 naming the fault when:

- the plan's base (the last complete run it diffed against) is not the
  record's (the record itself when its mode is complete, its `base` when
  incremental);
- a carried file's entry in the record does not name that run and sha (a
  carried figure whose base is not the last complete run: a file the record
  holds as an incremental night's figure, or as an older run's, is never
  carried again);
- on an incremental night, a floor of `MUTANTS_FLOORS` differs from the
  record's `floors` (a floor moves only on a complete run);
- an outcome names a file the plan did not mutate, or a complete plan does
  not mutate every floored file;
- the cargo pass's artifacts are not the dispatched matrix (#460): the count
  is the plan's (`shards` and `matrix`, the matrix the workflow dispatched,
  twenty on a complete night and the changed set's on an incremental one),
  never a number written here. Each shard k of the matrix must have its
  artifact `mutants-<k>` (an immediate subdirectory of a directory given)
  holding `shard-ok`, the marker its step writes only after cargo-mutants
  ended with 0, 2 or 3; a shard cut at the job's limit, failed or cancelled
  uploads its partial outcomes without it. The refusal names each shard with
  no artifact, each without its marker and each artifact outside the matrix.
  `--shards-only` makes that check alone (the score job's gate, before the
  survivors are read) and needs no `--record`.

An empty plan mutates nothing: the night carries every figure and says so,
and `--sha` has no shard to check. With `--run <url>` and `--write <out>` the
night's record is written: the record's shape (docs/scoreboard/mutants.json)
with `mode`, `base`, the night's `floors`, `matrix` (the plan's shard count
and max-parallel, #460), per file its counts and
`{"source": "night"|"carried", "run", "sha"}`, and the record's `verdict`
clause of each carried file (rule 1's exception, as tools/scoreboard reads it:
a carried figure keeps its clause; a file the night measured has none until
the owner gives one).
"""

import datetime
import json
import os
import re
import sys
from collections import Counter, defaultdict
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


def duplicated(args):
    """Each mutant name with more than one outcome, and its count, sorted."""
    seen = Counter(mutant["name"] for mutant, _ in mutant_outcomes(args))
    return sorted((name, n) for name, n in seen.items() if n > 1)


def malformed(passes):
    """The refusal's lines when a pass judged a name more than once, else []."""
    lines = []
    for label, dups in passes:
        if dups:
            lines += [
                f"**A malformed night (#400)**: {len(dups)} mutant name(s) have more than one outcome"
                f" in the {label}; a mutant is judged once; nothing is scored:",
                "",
            ]
            lines += [f"- `{name}`: {n} outcomes" for name, n in dups]
            lines += [""]
    return lines


def shard_shas(dirs):
    """Each shard's directory name and the sha its artifact names (None without one)."""
    for d in dirs:
        for shard in sorted(p for p in Path(d).iterdir() if p.is_dir()):
            sha = shard / "sha"
            yield shard.name, sha.read_text().strip() if sha.is_file() else None


def one_tree(dispatch, shards):
    """The refusal's lines when the shards are not all on the dispatch's commit, else []."""
    off = [(name, sha) for name, sha in shards if sha != dispatch]
    if not shards:
        return [f"**A night is one tree (#423)**: no shard's sha was found; the dispatch is `{dispatch}`; nothing is scored."]
    if not off:
        return []
    lines = [
        f"**A night is one tree (#423)**: the dispatch is `{dispatch}`, and {len(off)} of {len(shards)}"
        " artifact(s) (the shards and the survivors) name another commit or none; nothing is scored:",
        "",
    ]
    lines += [f"- `{name}`: {f'`{sha}`' if sha else 'no sha'}" for name, sha in off]
    return lines


def last_complete(record):
    """The last complete run a record names, {"run", "commit"}, or None."""
    if record.get("mode") == "complete":
        return {"run": record.get("run"), "commit": record.get("commit")}
    if record.get("mode") == "incremental" and isinstance(record.get("base"), dict):
        return {"run": record["base"].get("run"), "commit": record["base"].get("commit")}
    return None


def num(v):
    return "absent" if v is None else f"{float(v):g}"


def refuse_plan(plan, record, floors, outcome_files):
    """The refusal's lines for a night whose plan, record or floors do not hold, else []."""
    lines = []
    base = last_complete(record)
    if base is None:
        return [f"**No last complete run (#462)**: the record names no mode, or an incremental record no base; nothing is carried or scored."]
    if plan.get("base") != base:
        lines.append(f"**A stale base (#462)**: the plan diffed against {plan.get('base')}, the record's last complete run is {base}; nothing is scored.")
    floored = sorted(floors)
    planned = set(plan.get("files", []))
    if plan.get("mode") == "complete" and planned != set(floored):
        lines.append(f"**A complete night mutates every floored file (#462)**: the plan mutates {sorted(planned)} of {floored}.")
    if plan.get("mode") == "incremental":
        said = record.get("floors")
        moved = sorted(f for f in set(floors) | set(said or {}) if (said or {}).get(f) != floors.get(f))
        if moved:
            lines.append("**A floor moves only on a complete run (#462)**: on this incremental night "
                         + ", ".join(f"`{f}` is {num(floors.get(f))} against the last complete run's {num((said or {}).get(f))}" for f in moved)
                         + "; nothing is scored.")
    for f in floored:
        if f in planned:
            continue
        entry = record.get("files", {}).get(f)
        if entry is None:
            lines.append(f"**No figure to carry (#462)**: `{f}` is not mutated tonight and the record holds no figure for it.")
            continue
        named = {"run": entry.get("run", record.get("run")), "commit": entry.get("sha", record.get("commit"))}
        if named != base or entry.get("source", "night") == "carried" and record.get("mode") == "complete":
            lines.append(f"**A carried figure's base is not the last complete run (#462)**: `{f}` names run "
                         f"{named['run']} at `{named['commit']}`, the last complete run is {base['run']} at `{base['commit']}`; nothing is scored.")
    unplanned = sorted(outcome_files - planned)
    if unplanned:
        lines.append(f"**An outcome outside the plan (#462)**: {', '.join(f'`{f}`' for f in unplanned)} mutated, not in the plan's files; nothing is scored.")
    return lines


SHARD_DIR = re.compile(r"mutants-(\d+)")


def shard_refusal(plan, dirs):
    """The refusal's lines when the cargo pass's artifacts are not the
    dispatched matrix's, every shard with its marker, else [] (#460)."""
    shards, matrix = plan.get("shards"), plan.get("matrix")
    if not isinstance(shards, int) or matrix != list(range(shards)):
        return [f"**A plan without its matrix (#460)**: the plan names {shards!r} shard(s) and the matrix "
                f"{matrix!r}; the count is the dispatched matrix's, so nothing is scored."]
    found = {}
    for d in dirs:
        for sub in sorted(p for p in Path(d).iterdir() if p.is_dir()):
            m = SHARD_DIR.fullmatch(sub.name)
            if m:
                found[int(m.group(1))] = sub
    absent = [k for k in matrix if k not in found]
    unmarked = [k for k in matrix if k in found and not (found[k] / "shard-ok").is_file()]
    outside = sorted(k for k in found if k not in matrix)
    if not (absent or unmarked or outside):
        return []
    whole = len(matrix) - len(absent) - len(unmarked)
    lines = [f"**A partial night (#460)**: the dispatched matrix is {shards} shard(s), {whole} of them with "
             f"an artifact and its success marker; a partial night is never scored:", ""]
    lines += [f"- shard {k}: no artifact `mutants-{k}` (it never ran, or was cancelled before its upload)" for k in absent]
    lines += [f"- shard {k}: no success marker (cut at the job's limit, failed or cancelled; its partial "
              f"outcomes are evidence, never a score)" for k in unmarked]
    lines += [f"- `mutants-{k}`: an artifact outside the dispatched matrix" for k in outside]
    return lines


def write(text):
    sys.stdout.write(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write(text)


def main(args):
    hermetic = None
    hermetic_dir = None
    dispatch = None
    survivors_dir = None
    opts = {}
    shards_only = args[:1] == ["--shards-only"]
    if shards_only:
        args = args[1:]
    while args[:1] in (["--hermetic"], ["--sha"], ["--survivors"], ["--plan"], ["--record"], ["--run"], ["--write"]):
        if len(args) < 2:
            print(f"{args[0]} needs a value", file=sys.stderr)
            return 1
        if args[0] == "--hermetic":
            hermetic_dir = args[1]
        elif args[0] == "--sha":
            dispatch = args[1]
        elif args[0] == "--survivors":
            survivors_dir = args[1]
        else:
            opts[args[0][2:]] = args[1]
        args = args[2:]
    if survivors_dir is not None and dispatch is None:
        print("--survivors needs --sha", file=sys.stderr)
        return 1
    if shards_only:
        if "plan" not in opts or set(opts) - {"plan"} or dispatch or hermetic_dir or survivors_dir:
            print("--shards-only takes --plan and the cargo pass's directories alone", file=sys.stderr)
            return 1
        refusal = shard_refusal(json.loads(Path(opts["plan"]).read_text()), [a for a in args if Path(a).is_dir()])
        if refusal:
            write("\n".join(refusal) + "\n")
            return 1
        print("every shard of the dispatched matrix uploaded its artifact and its success marker")
        return 0
    if ("plan" in opts) != ("record" in opts) or ("write" in opts and "plan" not in opts):
        print("--plan and --record go together, and --write needs them", file=sys.stderr)
        return 1
    floors = {f: float(v) for f, v in json.loads(os.environ.get("MUTANTS_FLOORS", "{}")).items()}
    plan = record = None
    if "plan" in opts:
        plan = json.loads(Path(opts["plan"]).read_text())
        record = json.loads(Path(opts["record"]).read_text())
        refusal = refuse_plan(plan, record, floors, {m["file"] for m, _ in mutant_outcomes(args)})
        if not refusal and plan.get("files"):
            refusal = shard_refusal(plan, [a for a in args if Path(a).is_dir()])
        if refusal:
            write("\n".join(refusal) + "\n")
            return 1
    if dispatch is not None and plan is not None and not plan.get("files"):
        dispatch_checked = False
    else:
        dispatch_checked = dispatch is not None
    if dispatch_checked:
        shards = list(shard_shas([a for a in args if Path(a).is_dir()] + ([hermetic_dir] if hermetic_dir else [])))
        if survivors_dir is not None:
            sha = Path(survivors_dir) / "sha"
            shards.append((Path(survivors_dir).name, sha.read_text().strip() if sha.is_file() else None))
        refusal = one_tree(dispatch, shards)
        if refusal:
            write("\n".join(refusal) + "\n")
            return 1
    passes = [("cargo pass", duplicated(args))]
    if hermetic_dir is not None:
        passes.append(("hermetic pass", duplicated([hermetic_dir])))
    refusal = malformed(passes)
    if refusal:
        write("\n".join(refusal))
        return 1
    if hermetic_dir is not None:
        # Each name the hermetic pass judged, once: caught or not.
        hermetic = {
            mutant["name"]: summary == "CaughtMutant" for mutant, summary in mutant_outcomes([hermetic_dir])
        }
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
    carried = {}
    if plan is not None:
        base = last_complete(record)
        carried = {f: record["files"][f] for f in sorted(floors) if f not in set(plan.get("files", []))}
        lines += [f"A {plan['mode']} night (#462); the last complete run: {base['run']} at `{base['commit']}`."]
        if not plan.get("files"):
            lines += ["No floored file changed since it and no test or dependency reaches one: the night "
                      "mutated nothing and carries every file's figure."]
        elif carried:
            lines += ["A carried row is that run's figure, judged at that run and not again tonight."]
        lines += [""]
    if dispatch_checked:
        lines += [f"The night's commit: `{dispatch}` (the dispatch's; every shard ran on it, #423).", ""]
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
        if file in carried:
            e = carried[file]
            lines.append(
                f"| `{file}` | {e['caught']} | {e['timeout']} | {e['missed']} | {e['unviable']} | {e['score']:.1f} "
                f"| {floors.get(file, DEFAULT_FLOOR):g} | carried from run {e.get('run', record.get('run')).rsplit('/', 1)[-1]} "
                f"at `{e.get('sha', record.get('commit'))[:7]}` |" + (" |" if hermetic is not None else ""))
            continue
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
    for file in sorted(missed):
        names = sorted(missed[file])
        lines += ["", f"<details><summary>{len(names)} missed in <code>{file}</code></summary>", ""]
        lines += [f"- `{name}`" for name in names]
        lines += ["", "</details>"]
    if unjudged:
        failed = True
        lines += ["", f"**{len(unjudged)} survivor(s) of the cargo pass have no hermetic outcome**; no union:", ""]
        lines += [f"- `{name}`" for name in sorted(unjudged)]
    write("\n".join(lines) + "\n")
    if "write" in opts:
        night = {f: counts.get(f, {}) for f in plan.get("files", [])}
        out_record = night_record(plan, record, floors, night, carried, dispatch, opts.get("run"))
        Path(opts["write"]).write_text(json.dumps(out_record, indent=2) + "\n")
        print(f"wrote {opts['write']}")
    return 1 if failed else 0


def night_record(plan, record, floors, night, carried, commit, run):
    """The night's record, docs/scoreboard/mutants.json's shape, with each file's provenance."""
    base = last_complete(record)
    files = {}
    for f in sorted(floors):
        if f in carried:
            e = carried[f]
            files[f] = {k: e[k] for k in ("score", "caught", "missed", "timeout", "unviable")}
            files[f].update(source="carried", run=base["run"], sha=base["commit"])
            continue
        c = night.get(f, {})
        caught, missed, timeout = c.get("CaughtMutant", 0), c.get("MissedMutant", 0), c.get("Timeout", 0)
        judged = caught + missed + timeout
        files[f] = {"score": round(100.0 * caught / judged, 1) if judged else None, "caught": caught,
                    "missed": missed, "timeout": timeout, "unviable": c.get("Unviable", 0),
                    "source": "night", "run": run, "sha": commit}
    out = {"schema": 1, "mode": plan["mode"], "commit": commit, "run": run,
           "date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")}
    if plan["mode"] == "incremental":
        out["base"] = base
    out["floors"] = {f: int(v) if float(v).is_integer() else v for f, v in floors.items()}
    out["matrix"] = {"shards": plan.get("shards"), "parallel": plan.get("parallel")}
    said = verdict(record, carried)
    if said:
        out["verdict"] = said
    out["files"] = files
    return out


# The record's verdict (rule 1's exception), as tools/scoreboard reads it: the
# prefix, then a clause per file below its floor with the issue after it.
VERDICT_PREFIX = "red by design: "
VERDICT_CLAUSE = re.compile(r"(\S+) (\d+(?:\.\d+)?) below its floor (\d+(?:\.\d+)?)")


def verdict(record, carried):
    """The record's verdict clauses on the files the night carries, each with its
    words and issue, or None. A carried file keeps its figure and its floor, so
    its clause still holds (#464's review: without it the written record lost
    tracker_host.rs's exception and tools/scoreboard --check exited 1); a file
    the night measured is judged afresh and its clause is the owner's to give."""
    text = record.get("verdict")
    if not isinstance(text, str) or not text.startswith(VERDICT_PREFIX):
        return None
    found = list(VERDICT_CLAUSE.finditer(text))
    kept = [text[m.start():found[i + 1].start() if i + 1 < len(found) else len(text)].strip().rstrip(";").strip()
            for i, m in enumerate(found) if m.group(1) in carried]
    return VERDICT_PREFIX + "; ".join(kept) if kept else None


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
