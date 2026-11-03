#!/usr/bin/env python3
"""The planted cases of the Nightly's score in its two modes (#462),
.github/nightly/mutants_score.py with --plan and --record: an incremental
night scores the files it mutated and carries every other file's figure from
the last complete run, naming that run and sha in the table and in the record
it writes; it refuses a carried figure whose base is not the last complete
run (an older run's, or an incremental night's), a plan diffed against
another base, a floor moved on an incremental night (a complete night may
move one), an outcome outside the plan and a complete plan short of a floored
file; an empty plan carries everything and says so. The count is the
dispatched matrix's (#460): a twenty-shard complete night is scored and its
record names its matrix, nineteen artifacts of twenty are refused naming the
absent shard, as are a shard without its success marker (cut at the job's
limit) and an artifact outside the matrix, by the score and by its
--shards-only gate. A declared equivalent (#482, --equivalents) the night
missed is scored equivalent, outside the denominator and in its own column
and the written record; an entry the night caught, a stale entry (a name no
mutant of a mutated file carries) and a malformed entry are refused by name,
and an entry of a file the night carries is not judged. Each case writes its
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


def plan(files, mode="incremental", base=None, shards=None):
    shards = (1 if files else 0) if shards is None else shards
    return {"mode": mode, "base": base or {"run": RUN_A, "commit": SHA_A}, "head": HEAD,
            "floored": sorted(FLOORS), "files": files, "shards": shards, "matrix": list(range(shards)),
            "parallel": shards if mode == "complete" else min(6, shards)}


def equivalent(file, i, pr=434):
    return {"name": f"{file}:{i}: a mutant", "file": file, "line": i, "pr": pr,
            "reason": "no behaviour can tell it from the original", "review": "VERDICT merge, confidence 0.9"}


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

    def shard(self, n, outcomes, sha=HEAD, marker=True):
        self.put(f"outcomes/mutants-{n}/outcomes.json", {"outcomes": outcomes})
        self.put(f"outcomes/mutants-{n}/sha", sha + "\n")
        if marker:
            self.put(f"outcomes/mutants-{n}/shard-ok", f"shard {n} exit 2; cargo test --locked --lib\n")

    def gate(self, the_plan):
        """The score job's gate: --shards-only over the cargo pass's directory."""
        r = subprocess.run([sys.executable, "-B", SCRIPT, "--shards-only", "--plan", self.put("plan.json", the_plan),
                            os.path.join(self.root, "outcomes")], capture_output=True, text=True,
                           env={k: v for k, v in os.environ.items() if k != "GITHUB_STEP_SUMMARY"})
        return r.returncode, (r.stdout + r.stderr).strip()

    def score(self, the_plan, record, floors=FLOORS, outcomes=True, equivalents=None):
        args = [sys.executable, "-B", SCRIPT, "--sha", HEAD,
                "--plan", self.put("plan.json", the_plan), "--record", self.put("record.json", record),
                "--run", NIGHT, "--write", os.path.join(self.root, "night.json")]
        if equivalents is not None:
            args += ["--equivalents", self.put("equivalents.json", {"schema": 1, "equivalents": equivalents})]
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
        code, out = self.night.score(plan(["src/a.rs", "src/b.rs"], mode="complete", shards=2), complete_record(),
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

    def test_a_carried_file_keeps_its_verdict_clause(self):
        # #464's review: the written record lacked `verdict`, so committing it
        # lost tracker_host.rs's red-by-design exception and tools/scoreboard
        # --check exited 1. b (74.0 below its floor 75) is carried tonight and
        # keeps its clause, words and issue; a's clause (measured tonight) is
        # the owner's to give again.
        record = complete_record()
        record["verdict"] = ("red by design: src/a.rs 95.0 below its floor 96; its survivors are #1; "
                             "src/b.rs 74.0 below its floor 75; the floor does not move; its survivors are #175's")
        self.a_night_on_a()
        code, out = self.night.score(plan(["src/a.rs"]), record)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.night.written()["verdict"],
                         "red by design: src/b.rs 74.0 below its floor 75; the floor does not move; "
                         "its survivors are #175's")
        # Nothing carried with a clause, or no verdict: none written.
        code, out = self.night.score(plan(["src/a.rs", "src/b.rs"], mode="complete"), record)
        self.assertNotIn("verdict", self.night.written())
        del record["verdict"]
        self.night.score(plan(["src/a.rs"]), record)
        self.assertNotIn("verdict", self.night.written())

    def twenty_shards(self, skip=()):
        # A complete night's twenty shards (#460), each 2 mutants of a and 2 of
        # b dealt round-robin: 40 of a (38 caught), 40 of b (32 caught).
        for k in range(20):
            if k in skip:
                continue
            self.night.shard(k, [outcome("src/a.rs", k, "CaughtMutant"),
                                 outcome("src/a.rs", 20 + k, "MissedMutant" if k < 2 else "CaughtMutant"),
                                 outcome("src/b.rs", k, "CaughtMutant"),
                                 outcome("src/b.rs", 20 + k, "MissedMutant" if k < 8 else "CaughtMutant")])

    def test_a_twenty_shard_complete_night_is_scored(self):
        self.twenty_shards()
        the_plan = plan(["src/a.rs", "src/b.rs"], mode="complete", shards=20)
        self.assertEqual(self.night.gate(the_plan)[0], 0)
        code, out = self.night.score(the_plan, complete_record())
        self.assertEqual(code, 0, out)
        self.assertIn("| `src/a.rs` | 38 | 0 | 2 | 0 | 95.0 | 90 | ok |", out)
        self.assertIn("| `src/b.rs` | 32 | 0 | 8 | 0 | 80.0 | 75 | ok |", out)
        self.assertEqual(self.night.written()["matrix"], {"shards": 20, "parallel": 20})
        print("\n    exit 0: twenty shards of twenty, src/a.rs 95.0, src/b.rs 80.0, matrix 20 at once",
              end=" ", file=sys.stderr)

    def test_a_complete_night_of_nineteen_shards_is_refused(self):
        # Twenty dispatched, nineteen artifacts: shard 13 never uploaded.
        self.twenty_shards(skip=(13,))
        the_plan = plan(["src/a.rs", "src/b.rs"], mode="complete", shards=20)
        self.refused("- shard 13: no artifact `mutants-13`", the_plan, complete_record())
        code, out = self.night.gate(the_plan)
        self.assertEqual(code, 1, out)
        self.assertIn("the dispatched matrix is 20 shard(s), 19 of them with an artifact and its success marker", out)
        # The same nineteen are whole against a plan of nineteen: the count is the plan's, never a constant.
        self.assertEqual(self.night.gate(plan(["src/a.rs", "src/b.rs"], mode="complete", shards=19))[0], 1)
        self.assertIn("`mutants-19`: an artifact outside the dispatched matrix",
                      self.night.gate(plan(["src/a.rs", "src/b.rs"], mode="complete", shards=19))[1])

    def test_a_shard_cut_at_the_job_limit_is_refused_by_name(self):
        # The shard's step never wrote its marker (the job's limit cut it);
        # its partial outcomes were uploaded under always().
        self.twenty_shards(skip=(7,))
        self.night.shard(7, [outcome("src/a.rs", 7, "CaughtMutant")], marker=False)
        the_plan = plan(["src/a.rs", "src/b.rs"], mode="complete", shards=20)
        self.refused("- shard 7: no success marker (cut at the job's limit", the_plan, complete_record())
        self.assertEqual(self.night.gate(the_plan)[0], 1)

    def test_a_plan_without_its_matrix_is_refused(self):
        self.a_night_on_a()
        the_plan = plan(["src/a.rs"])
        del the_plan["matrix"]
        self.refused("A plan without its matrix (#460)", the_plan, complete_record())

    def test_an_incremental_night_counts_its_own_shards(self):
        # The changed set's six shards (#462's sizing), all six whole.
        for k in range(6):
            self.night.shard(k, [outcome("src/a.rs", k, "CaughtMutant")])
        code, out = self.night.score(plan(["src/a.rs"], shards=6), complete_record())
        self.assertEqual(code, 0, out)
        self.assertEqual(self.night.written()["matrix"], {"shards": 6, "parallel": 6})

    def test_a_declared_equivalent_is_excluded_from_the_denominator(self):
        # 19 caught, 3 missed; 102 is declared equivalent: 19 / 21 = 90.5 at the floor 90,
        # where 19 / 22 = 86.4 would be below it.
        self.a_night_on_a(caught=19, missed=3)
        code, out = self.night.score(plan(["src/a.rs"]), complete_record(), equivalents=[equivalent("src/a.rs", 102)])
        self.assertEqual(code, 0, out)
        self.assertIn("| `src/a.rs` | 19 | 0 | 2 | 1 | 0 | 90.5 | 90 | ok |", out)
        self.assertIn("1 equivalent in <code>src/a.rs</code>", out)
        self.assertIn("- `src/a.rs:102: a mutant` (PR 434)", out)
        self.assertIn("2 missed in <code>src/a.rs</code>", out)
        a = self.night.written()["files"]["src/a.rs"]
        self.assertEqual((a["score"], a["caught"], a["missed"], a["equivalent"]), (90.5, 19, 2, 1))
        # The carried file's row reads its own count, 0 when its entry names none.
        self.assertIn("| `src/b.rs` | 74 | 0 | 26 | 0 | 1 | 74.0 | 75 | carried", out)
        self.assertNotIn("equivalent", self.night.written()["files"]["src/b.rs"])
        # Without --equivalents the same night is 86.4, below its floor, and has no column.
        code, out = self.night.score(plan(["src/a.rs"]), complete_record())
        self.assertEqual(code, 1, out)
        self.assertIn("| `src/a.rs` | 19 | 0 | 3 | 0 | 86.4 | 90 | below the floor |", out)
        print("\n    exit 0: src/a.rs 90.5 with 1 equivalent (86.4 and exit 1 without)", end=" ", file=sys.stderr)

    def test_a_caught_equivalent_is_refused_by_name(self):
        self.a_night_on_a(caught=19, missed=1)
        self.refused("- `src/a.rs:3: a mutant` (declared in PR 467)", plan(["src/a.rs"]), complete_record(),
                     equivalents=[equivalent("src/a.rs", 100), equivalent("src/a.rs", 3, pr=467)])
        self.assertIn("A declared equivalent was caught (#482)", self.night.score(
            plan(["src/a.rs"]), complete_record(), equivalents=[equivalent("src/a.rs", 3, pr=467)])[1])

    def test_a_stale_equivalent_is_refused_by_name(self):
        # The night mutated src/a.rs and lists no mutant at line 500: the entry moved.
        self.a_night_on_a()
        self.refused("- `src/a.rs:500: a mutant` (declared in PR 475)", plan(["src/a.rs"]), complete_record(),
                     equivalents=[equivalent("src/a.rs", 500, pr=475)])
        self.assertIn("A stale equivalent (#482)", self.night.score(
            plan(["src/a.rs"]), complete_record(), equivalents=[equivalent("src/a.rs", 500, pr=475)])[1])

    def test_an_equivalent_of_a_carried_file_is_not_judged(self):
        # src/b.rs is carried tonight: its entry names no mutant of the night and is not stale.
        self.a_night_on_a()
        code, out = self.night.score(plan(["src/a.rs"]), complete_record(),
                                     equivalents=[equivalent("src/b.rs", 7)])
        self.assertEqual(code, 0, out)

    def test_a_malformed_equivalent_is_refused(self):
        self.a_night_on_a()
        bad = dict(equivalent("src/a.rs", 100), line=99)
        self.refused("does not begin with its file and line, `src/a.rs:99:`", plan(["src/a.rs"]),
                     complete_record(), equivalents=[bad])
        self.refused("- `src/a.rs:100: a mutant`: named twice", plan(["src/a.rs"]), complete_record(),
                     equivalents=[equivalent("src/a.rs", 100)] * 2)
        no_review = {k: v for k, v in equivalent("src/a.rs", 100).items() if k != "review"}
        self.refused("- entry 0: `review` absent or empty", plan(["src/a.rs"]), complete_record(),
                     equivalents=[no_review])

    def test_the_committed_equivalents_read_whole(self):
        # The tree's own record: six entries, each from a batch PR with its review's verdict.
        sys.path.insert(0, os.path.dirname(SCRIPT))
        try:
            import mutants_score
        finally:
            sys.path.pop(0)
        entries, refusal = mutants_score.load_equivalents(
            os.path.join(os.path.dirname(HERE), "docs", "scoreboard", "equivalents.json"))
        self.assertEqual(refusal, [])
        self.assertEqual(len(entries), 6)
        self.assertEqual(sorted({e["pr"] for e in entries.values()}), [434, 467, 475])
        for name, e in entries.items():
            self.assertTrue(e["review"].startswith("VERDICT merge"), name)

    def test_the_committed_record_is_a_complete_base(self):
        # The tree's own record: the last complete run, every file the night's.
        with open(os.path.join(os.path.dirname(HERE), "docs", "scoreboard", "mutants.json")) as f:
            record = json.load(f)
        self.assertEqual(record["mode"], "complete")
        for file, e in record["files"].items():
            self.assertEqual((e["source"], e["run"], e["sha"]), ("night", record["run"], record["commit"]), file)


if __name__ == "__main__":
    unittest.main(verbosity=2)
