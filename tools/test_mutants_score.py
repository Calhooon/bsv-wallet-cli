#!/usr/bin/env python3
"""The planted cases of the Nightly's score in its two modes (#462),
.github/nightly/mutants_score.py with --plan and --record: an incremental
night scores the files it mutated and carries every other file's figure from
the last complete run, naming that run and sha in the table and in the record
it writes; it refuses a carried figure whose base is not the last complete
run (an older run's, or an incremental night's), a plan diffed against
another base, a floor moved on an incremental night (a complete night may
move one), an outcome outside the plan and a complete plan short of a floored
file; an empty plan carries everything and says so. Each case writes its
inputs under a directory of its own in $TMPDIR. Run by the CI job ratchet:
python3 tools/test_mutants_score.py
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), ".github", "nightly", "mutants_score.py")

RUN_A = "https://github.com/Calhooon/bsv-wallet-cli/actions/runs/1000"
RUN_B = "https://github.com/Calhooon/bsv-wallet-cli/actions/runs/2000"
NIGHT = "https://github.com/Calhooon/bsv-wallet-cli/actions/runs/3000"
SHA_A = "a" * 40
SHA_B = "b" * 40
HEAD = "c" * 40
FLOORS = {"src/a.rs": 90, "src/b.rs": 75}


def entry(caught, missed, run=RUN_A, sha=SHA_A, source="night"):
    return {"score": round(100.0 * caught / (caught + missed), 1), "caught": caught, "missed": missed,
            "timeout": 0, "unviable": 1, "source": source, "run": run, "sha": sha}


def complete_record():
    return {"schema": 1, "mode": "complete", "commit": SHA_A, "run": RUN_A, "date": "2026-11-02",
            "floors": dict(FLOORS), "files": {"src/a.rs": entry(95, 5), "src/b.rs": entry(74, 26)}}


def plan(files, mode="incremental", base=None):
    return {"mode": mode, "base": base or {"run": RUN_A, "commit": SHA_A}, "head": HEAD,
            "floored": sorted(FLOORS), "files": files, "shards": 1 if files else 0}


def outcome(file, i, summary):
    return {"scenario": {"Mutant": {"name": f"{file}:{i}: a mutant", "file": file}}, "summary": summary}


class Night:
    def __init__(self):
        self.root = tempfile.mkdtemp(prefix="mutants-score-case.", dir=os.environ.get("TMPDIR"))

    def put(self, name, value):
        path = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(value if isinstance(value, str) else json.dumps(value))
        return path

    def shard(self, n, outcomes, sha=HEAD):
        self.put(f"outcomes/mutants-{n}/outcomes.json", {"outcomes": outcomes})
        self.put(f"outcomes/mutants-{n}/sha", sha + "\n")

    def score(self, the_plan, record, floors=FLOORS, outcomes=True):
        args = [sys.executable, "-B", SCRIPT, "--sha", HEAD,
                "--plan", self.put("plan.json", the_plan), "--record", self.put("record.json", record),
                "--run", NIGHT, "--write", os.path.join(self.root, "night.json")]
        if outcomes:
            args.append(os.path.join(self.root, "outcomes"))
        env = dict(os.environ, MUTANTS_FLOORS=json.dumps(floors))
        env.pop("GITHUB_STEP_SUMMARY", None)
        r = subprocess.run(args, capture_output=True, text=True, env=env)
        return r.returncode, (r.stdout + r.stderr).strip()

    def written(self):
        with open(os.path.join(self.root, "night.json")) as f:
            return json.load(f)

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


class Case(unittest.TestCase):
    def setUp(self):
        self.night = Night()

    def tearDown(self):
        self.night.cleanup()

    def a_night_on_a(self, caught=19, missed=1):
        self.night.shard(0, [outcome("src/a.rs", i, "CaughtMutant") for i in range(caught)]
                         + [outcome("src/a.rs", 100 + i, "MissedMutant") for i in range(missed)])

    def refused(self, needle, *args, **kw):
        code, out = self.night.score(*args, **kw)
        self.assertEqual(code, 1, out)
        self.assertIn(needle, out)
        self.assertNotIn("| file |", out)  # refused before any figure
        print(f"\n    exit 1: {[l for l in out.splitlines() if needle in l][0][:150]}", end=" ", file=sys.stderr)

    def test_an_incremental_night_scores_its_file_and_carries_the_rest(self):
        self.a_night_on_a()
        code, out = self.night.score(plan(["src/a.rs"]), complete_record())
        self.assertEqual(code, 0, out)
        self.assertIn("| `src/a.rs` | 19 | 0 | 1 | 0 | 95.0 | 90 | ok |", out)
        self.assertIn("| `src/b.rs` | 74 | 0 | 26 | 1 | 74.0 | 75 | carried from run 1000 at `aaaaaaa` |", out)
        rec = self.night.written()
        self.assertEqual(rec["mode"], "incremental")
        self.assertEqual(rec["base"], {"run": RUN_A, "commit": SHA_A})
        self.assertEqual({k: rec["files"]["src/a.rs"][k] for k in ("source", "run", "sha", "score")},
                         {"source": "night", "run": NIGHT, "sha": HEAD, "score": 95.0})
        self.assertEqual({k: rec["files"]["src/b.rs"][k] for k in ("source", "run", "sha", "score")},
                         {"source": "carried", "run": RUN_A, "sha": SHA_A, "score": 74.0})
        print("\n    exit 0: src/a.rs the night's 95.0, src/b.rs carried 74.0 from run 1000", end=" ", file=sys.stderr)

    def test_a_night_file_below_its_floor_fails_a_carried_one_is_not_judged_again(self):
        self.a_night_on_a(caught=17, missed=3)
        code, out = self.night.score(plan(["src/a.rs"]), complete_record())
        self.assertEqual(code, 1, out)
        self.assertIn("| `src/a.rs` | 17 | 0 | 3 | 0 | 85.0 | 90 | below the floor |", out)

    def test_an_empty_plan_carries_everything_and_says_so(self):
        code, out = self.night.score(plan([]), complete_record(), outcomes=False)
        self.assertEqual(code, 0, out)
        self.assertIn("the night mutated nothing and carries every file's figure", out)
        self.assertEqual({f: e["source"] for f, e in self.night.written()["files"].items()},
                         {"src/a.rs": "carried", "src/b.rs": "carried"})

    def test_a_carried_figure_from_an_older_run_is_refused(self):
        record = complete_record()
        record["files"]["src/b.rs"] = entry(74, 26, run=RUN_B, sha=SHA_B)
        self.a_night_on_a()
        self.refused("A carried figure's base is not the last complete run (#462)**: `src/b.rs` names run "
                     f"{RUN_B}", plan(["src/a.rs"]), record)

    def test_an_incremental_nights_figure_is_never_carried(self):
        # The record is an incremental night's (base A); src/b.rs is that
        # night's own figure, so tonight, not mutating it, may not carry it.
        record = complete_record()
        record.update(mode="incremental", commit=SHA_B, run=RUN_B, base={"run": RUN_A, "commit": SHA_A})
        record["files"]["src/b.rs"] = entry(80, 20, run=RUN_B, sha=SHA_B)
        record["files"]["src/a.rs"] = entry(95, 5, source="carried")
        self.a_night_on_a()
        self.refused("`src/b.rs` names run " + RUN_B, plan(["src/a.rs"]), record)

    def test_a_plan_against_a_stale_base_is_refused(self):
        self.a_night_on_a()
        self.refused("A stale base (#462)", plan(["src/a.rs"], base={"run": RUN_B, "commit": SHA_B}),
                     complete_record())

    def test_a_floor_moved_on_an_incremental_night_is_refused(self):
        self.a_night_on_a()
        self.refused("A floor moves only on a complete run (#462)**: on this incremental night `src/b.rs` is 70 against the last complete run's 75",
                     plan(["src/a.rs"]), complete_record(), floors={"src/a.rs": 90, "src/b.rs": 70})

    def test_a_floor_moved_on_a_complete_night_is_scored(self):
        self.a_night_on_a()
        self.night.shard(1, [outcome("src/b.rs", i, "CaughtMutant") for i in range(9)]
                         + [outcome("src/b.rs", 50, "MissedMutant")])
        code, out = self.night.score(plan(["src/a.rs", "src/b.rs"], mode="complete"), complete_record(),
                                     floors={"src/a.rs": 90, "src/b.rs": 80})
        self.assertEqual(code, 0, out)
        self.assertNotIn("carried", out)
        self.assertEqual({f: e["source"] for f, e in self.night.written()["files"].items()},
                         {"src/a.rs": "night", "src/b.rs": "night"})

    def test_a_complete_plan_short_of_a_floored_file_is_refused(self):
        self.a_night_on_a()
        self.refused("A complete night mutates every floored file", plan(["src/a.rs"], mode="complete"),
                     complete_record())

    def test_an_outcome_outside_the_plan_is_refused(self):
        self.a_night_on_a()
        self.night.shard(1, [outcome("src/b.rs", 1, "CaughtMutant")])
        self.refused("An outcome outside the plan (#462)**: `src/b.rs`", plan(["src/a.rs"]), complete_record())

    def test_a_record_without_a_mode_carries_nothing(self):
        record = complete_record()
        del record["mode"]
        self.a_night_on_a()
        self.refused("No last complete run (#462)", plan(["src/a.rs"]), record)

    def test_the_committed_record_is_a_complete_base(self):
        # The tree's own record: the last complete run, every file the night's.
        with open(os.path.join(os.path.dirname(HERE), "docs", "scoreboard", "mutants.json")) as f:
            record = json.load(f)
        self.assertEqual(record["mode"], "complete")
        for file, e in record["files"].items():
            self.assertEqual((e["source"], e["run"], e["sha"]), ("night", record["run"], record["commit"]), file)


if __name__ == "__main__":
    unittest.main(verbosity=2)
