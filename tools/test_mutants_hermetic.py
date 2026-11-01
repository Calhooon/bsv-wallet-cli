#!/usr/bin/env python3
"""The planted cases of the Nightly's hermetic pass (#393):
.github/nightly/mutants_hermetic.py's survivors (missed and timeout in,
caught, unviable and the baseline out, a name of two copies once), its
round-robin shards and anchored escapes, its list check; and
.github/nightly/mutants_score.py's union with `--hermetic` (a survivor the
hermetic pass caught is caught, one it missed stays missed, a timeout it
caught is caught, a survivor it never judged fails the score), the plain
score unchanged without the flag. Each case writes its files in a directory
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


# The cargo pass, two shards: one of each outcome, a baseline, and a name of
# explorers.rs's two copies (the library's and the binary's) missed twice.
CARGO = [
    [{"scenario": "Baseline", "summary": "Success"},
     outcome(GATE, "src/spend_gate.rs:1:1: replace a -> bool with true", "CaughtMutant"),
     outcome(GATE, "src/spend_gate.rs:2:1: replace && with || in b", "MissedMutant"),
     outcome(GATE, "src/spend_gate.rs:3:1: replace c -> u8 with 0", "Unviable"),
     outcome(GATE, "src/spend_gate.rs:4:1: replace <impl Display for E>::fmt -> Result with Ok(Default::default())", "Timeout")],
    [{"scenario": "Baseline", "summary": "Success"},
     outcome(EXPL, "src/explorers.rs:5:9: delete ! in d", "MissedMutant"),
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

    def plant(self, root, shards):
        for i, outcomes in enumerate(shards):
            os.makedirs(self.path(root, f"shard-{i}"))
            with open(self.path(root, f"shard-{i}", "outcomes.json"), "w") as f:
                json.dump({"outcomes": outcomes}, f)

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
        for listed, code in (("a\nb\nb\n", 0), ("a\n", 1), ("a\nb\nc\n", 1)):
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
        self.assertEqual(rows[EXPL][:5], ["0", "0", "3", "0", "0.0"])

    def test_the_union_counts_a_hermetic_catch_as_caught(self):
        hermetic = [[
            {"scenario": "Baseline", "summary": "Success"},
            outcome(GATE, SURVIVORS[2], "MissedMutant"),        # spend_gate b: stays missed
            outcome(GATE, SURVIVORS[3], "CaughtMutant"),        # spend_gate fmt: a timeout, caught
            outcome(EXPL, SURVIVORS[0], "MissedMutant"),        # explorers d, copy one
            outcome(EXPL, SURVIVORS[0], "CaughtMutant"),        # explorers d, copy two: caught
            outcome(EXPL, SURVIVORS[1], "Timeout"),             # explorers e: stays missed
        ]]
        r, rows = self.score(hermetic)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("The union of two passes", r.stdout)
        # caught, timeout, missed, unviable, score, floor, mark, by e2e_hermetic
        self.assertEqual(rows[GATE], ["2", "0", "1", "1", "66.7", "0", "ok", "1"])
        self.assertEqual(rows[EXPL], ["2", "0", "1", "0", "66.7", "0", "ok", "2"])
        self.assertNotIn("delete ! in d", r.stdout)

    def test_a_survivor_the_hermetic_pass_never_judged_fails_the_score(self):
        hermetic = [[outcome(GATE, n, "CaughtMutant") for n in SURVIVORS if n != SURVIVORS[1]]]
        r, rows = self.score(hermetic)
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("1 survivor(s) of the cargo pass have no hermetic outcome", r.stdout)
        self.assertIn(SURVIVORS[1], r.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
