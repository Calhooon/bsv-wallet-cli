#!/usr/bin/env python3
"""The planted cases of the Nightly's hermetic pass (#393):
.github/nightly/mutants_hermetic.py's survivors (missed and timeout in,
caught, unviable and the baseline out), its
round-robin shards and anchored escapes, its list check (a name listed
twice refused, #400); and
.github/nightly/mutants_score.py's union with `--hermetic` (a survivor the
hermetic pass caught is caught, one it missed stays missed, a timeout it
caught is caught, a survivor it never judged fails the score), the plain
score unchanged without the flag; and the score's one tree with `--sha`
(#423: two shards on different commits refused, naming each; a shard or the
survivors without a sha refused; all on the dispatch's commit accepted, the
commit printed above the table); and the score's one judgment a mutant
(#400: a name with two outcomes in the cargo pass or in the hermetic pass
refused, naming it and its count; unique names accepted). Each case writes its files in a directory
of its own under $TMPDIR. Run by the CI job ratchet:
python3 tools/test_mutants_hermetic.py
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
NIGHTLY = os.path.join(os.path.dirname(HERE), ".github", "nightly")
HERMETIC = os.path.join(NIGHTLY, "mutants_hermetic.py")
SCORE = os.path.join(NIGHTLY, "mutants_score.py")

GATE = "src/spend_gate.rs"
EXPL = "src/explorers.rs"


def outcome(file, name, summary):
    return {"scenario": {"Mutant": {"file": file, "name": name}}, "summary": summary}


# The cargo pass, two shards: one of each outcome, a baseline, each name once.
CARGO = [
    [{"scenario": "Baseline", "summary": "Success"},
     outcome(GATE, "src/spend_gate.rs:1:1: replace a -> bool with true", "CaughtMutant"),
     outcome(GATE, "src/spend_gate.rs:2:1: replace && with || in b", "MissedMutant"),
     outcome(GATE, "src/spend_gate.rs:3:1: replace c -> u8 with 0", "Unviable"),
     outcome(GATE, "src/spend_gate.rs:4:1: replace <impl Display for E>::fmt -> Result with Ok(Default::default())", "Timeout")],
    [{"scenario": "Baseline", "summary": "Success"},
     outcome(EXPL, "src/explorers.rs:5:9: delete ! in d", "MissedMutant"),
     outcome(EXPL, "src/explorers.rs:6:1: replace e -> String with \"xyzzy\".into()", "MissedMutant")],
]
SURVIVORS = sorted([
    "src/spend_gate.rs:2:1: replace && with || in b",
    "src/spend_gate.rs:4:1: replace <impl Display for E>::fmt -> Result with Ok(Default::default())",
    "src/explorers.rs:5:9: delete ! in d",
    "src/explorers.rs:6:1: replace e -> String with \"xyzzy\".into()",
])


class Case(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="test-mutants-hermetic-")

    def tearDown(self):
        shutil.rmtree(self.dir)

    def path(self, *parts):
        return os.path.join(self.dir, *parts)

    def plant(self, root, shards, shas=None):
        for i, outcomes in enumerate(shards):
            os.makedirs(self.path(root, f"shard-{i}"))
            with open(self.path(root, f"shard-{i}", "outcomes.json"), "w") as f:
                json.dump({"outcomes": outcomes}, f)
            if shas is not None and shas[i] is not None:
                with open(self.path(root, f"shard-{i}", "sha"), "w") as f:
                    f.write(shas[i] + "\n")

    def run_py(self, *args, env=None):
        env = {k: v for k, v in (env or os.environ).items() if k != "GITHUB_STEP_SUMMARY"}
        return subprocess.run([sys.executable, "-B", *args], capture_output=True, text=True, env=env)

    def lines(self, name):
        with open(self.path(name)) as f:
            return f.read().splitlines()

    def test_survivors_are_missed_and_timeout_once_each(self):
        self.plant("cargo", CARGO)
        r = self.run_py(HERMETIC, "survivors", self.path("cargo"), self.path("survivors.txt"))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.lines("survivors.txt"), SURVIVORS)
        self.assertIn("4 survivors", r.stdout)

    def test_shards_deal_round_robin_and_escape_to_exact_matches(self):
        with open(self.path("survivors.txt"), "w") as f:
            f.write("".join(f"{n}\n" for n in SURVIVORS))
        dealt = []
        for shard in range(3):
            r = self.run_py(HERMETIC, "shard", self.path("survivors.txt"), str(shard), "3",
                            self.path(f"names-{shard}"), self.path(f"re-{shard}"))
            self.assertEqual(r.returncode, 0, r.stderr)
            names, regexes = self.lines(f"names-{shard}"), self.lines(f"re-{shard}")
            self.assertEqual(names, SURVIVORS[shard::3])
            for name, rx in zip(names, regexes, strict=True):
                # Python's re reads these escapes as Rust's regex does.
                matched = [n for n in SURVIVORS + ["x" + name, name + "x"] if re.search(rx, n)]
                self.assertEqual(matched, [name], rx)
            dealt += names
        self.assertEqual(sorted(dealt), SURVIVORS)
        r = self.run_py(HERMETIC, "shard", self.path("survivors.txt"), "3", "3", self.path("n"), self.path("r"))
        self.assertEqual(r.returncode, 1)

    def test_the_list_check_refuses_any_other_selection(self):
        with open(self.path("names"), "w") as f:
            f.write("a\nb\n")
        for listed, code in (("a\nb\n", 0), ("a\nb\nb\n", 1), ("a\n", 1), ("a\nb\nc\n", 1)):
            with open(self.path("listed"), "w") as f:
                f.write(listed)
            r = self.run_py(HERMETIC, "check-list", self.path("names"), self.path("listed"))
            self.assertEqual(r.returncode, code, listed + r.stdout)

    def score(self, hermetic):
        self.plant("cargo", CARGO)
        args = [SCORE]
        if hermetic is not None:
            self.plant("hermetic", hermetic)
            args += ["--hermetic", self.path("hermetic")]
        env = dict(os.environ, MUTANTS_FLOORS=json.dumps({GATE: 0, EXPL: 0}))
        r = self.run_py(*args, self.path("cargo"), env=env)
        rows = {l.split("|")[1].strip(" `"): [c.strip() for c in l.split("|")[2:-1]]
                for l in r.stdout.splitlines() if l.startswith("| `")}
        return r, rows

    def test_without_the_flag_the_score_is_the_cargo_pass(self):
        r, rows = self.score(None)
        self.assertEqual(r.returncode, 0, r.stdout)
        # caught, timeout, missed, unviable, score
        self.assertEqual(rows[GATE][:5], ["1", "1", "1", "1", "33.3"])
        self.assertEqual(rows[EXPL][:5], ["0", "0", "2", "0", "0.0"])

    def test_the_union_counts_a_hermetic_catch_as_caught(self):
        hermetic = [[
            {"scenario": "Baseline", "summary": "Success"},
            outcome(GATE, SURVIVORS[2], "MissedMutant"),        # spend_gate b: stays missed
            outcome(GATE, SURVIVORS[3], "CaughtMutant"),        # spend_gate fmt: a timeout, caught
            outcome(EXPL, SURVIVORS[0], "CaughtMutant"),        # explorers d: caught
            outcome(EXPL, SURVIVORS[1], "Timeout"),             # explorers e: stays missed
        ]]
        r, rows = self.score(hermetic)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("The union of two passes", r.stdout)
        # caught, timeout, missed, unviable, score, floor, mark, by e2e_hermetic
        self.assertEqual(rows[GATE], ["2", "0", "1", "1", "66.7", "0", "ok", "1"])
        self.assertEqual(rows[EXPL], ["1", "0", "1", "0", "50.0", "0", "ok", "1"])
        self.assertNotIn("delete ! in d", r.stdout)

    def test_a_survivor_the_hermetic_pass_never_judged_fails_the_score(self):
        hermetic = [[outcome(GATE, n, "CaughtMutant") for n in SURVIVORS if n != SURVIVORS[1]]]
        r, rows = self.score(hermetic)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("1 survivor(s) of the cargo pass have no hermetic outcome", r.stdout)
        self.assertIn(SURVIVORS[1], r.stdout)


    # The hermetic pass of the union case above: every survivor judged.
    HERMETIC_ALL = [[outcome(GATE, n, "CaughtMutant") for n in SURVIVORS]]

    def night(self, cargo_shas, hermetic_shas, survivors_sha):
        self.plant("cargo", CARGO, cargo_shas)
        self.plant("hermetic", self.HERMETIC_ALL, hermetic_shas)
        os.makedirs(self.path("survivors"))
        if survivors_sha is not None:
            with open(self.path("survivors", "sha"), "w") as f:
                f.write(survivors_sha + "\n")
        env = dict(os.environ, MUTANTS_FLOORS=json.dumps({GATE: 0, EXPL: 0}))
        return self.run_py(SCORE, "--sha", DISPATCH, "--survivors", self.path("survivors"),
                           "--hermetic", self.path("hermetic"), self.path("cargo"), env=env)

    def test_a_night_on_the_dispatch_commit_is_scored_and_names_it(self):
        r = self.night([DISPATCH, DISPATCH], [DISPATCH], DISPATCH)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn(f"The night's commit: `{DISPATCH}`", r.stdout)
        self.assertLess(r.stdout.index(DISPATCH), r.stdout.index("| file |"))

    def test_two_shards_on_different_commits_are_refused_by_name(self):
        r = self.night([DISPATCH, OTHER], [DISPATCH], DISPATCH)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("A night is one tree", r.stdout)
        self.assertIn(f"- `shard-1`: `{OTHER}`", r.stdout)
        self.assertNotIn("`shard-0`", r.stdout)
        self.assertNotIn("| file |", r.stdout)  # no figure is judged

    def test_shards_all_on_one_commit_not_the_dispatch_are_refused(self):
        r = self.night([OTHER, OTHER], [OTHER], OTHER)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("4 of 4 artifact(s)", r.stdout)
        self.assertNotIn("| file |", r.stdout)

    def test_a_shard_without_a_sha_is_refused(self):
        r = self.night([DISPATCH, DISPATCH], [None], DISPATCH)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("- `shard-0`: no sha", r.stdout)
        self.assertNotIn("| file |", r.stdout)

    def test_survivors_without_a_sha_are_refused(self):
        r = self.night([DISPATCH, DISPATCH], [DISPATCH], None)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("- `survivors`: no sha", r.stdout)

    # A mutant is judged once (#400): the name cargo-mutants 27.1.0 listed
    # twice while main.rs and lib.rs both declared `mod explorers;`.
    TWICE = "src/explorers.rs:5:9: delete ! in d"

    def test_a_name_judged_twice_in_the_cargo_pass_is_refused(self):
        self.plant("cargo", CARGO + [[outcome(EXPL, self.TWICE, "MissedMutant")]])
        env = dict(os.environ, MUTANTS_FLOORS=json.dumps({GATE: 0, EXPL: 0}))
        r = self.run_py(SCORE, self.path("cargo"), env=env)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("A malformed night (#400)", r.stdout)
        self.assertIn("1 mutant name(s) have more than one outcome in the cargo pass", r.stdout)
        self.assertIn(f"- `{self.TWICE}`: 2 outcomes", r.stdout)
        self.assertNotIn("| file |", r.stdout)  # no figure is judged

    def test_a_name_judged_twice_in_the_hermetic_pass_is_refused(self):
        hermetic = [[outcome(GATE, n, "CaughtMutant") for n in SURVIVORS],
                    [outcome(EXPL, self.TWICE, "MissedMutant")]]
        r, rows = self.score(hermetic)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("more than one outcome in the hermetic pass", r.stdout)
        self.assertNotIn("in the cargo pass", r.stdout)
        self.assertEqual(rows, {})

    def test_each_name_judged_once_is_scored(self):
        r, rows = self.score(self.HERMETIC_ALL)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertNotIn("malformed", r.stdout)
        self.assertEqual(rows[EXPL][:5], ["2", "0", "0", "0", "100.0"])


DISPATCH = "6bf9a2c" + "0" * 33
OTHER = "54c7276" + "0" * 33


if __name__ == "__main__":
    unittest.main(verbosity=2)
