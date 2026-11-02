#!/usr/bin/env python3
"""The planted cases of the guard on the Mutants shards' left-out targets
(#401), .github/nightly/no_floored.py: `list` prints one kind's names from
the one list; `check` passes a record holding every entry at zero, and fails
naming the target and the floored line it runs, an entry the record does not
measure, a record entry the list does not hold, an entry naming no target, a
floored file the record does not measure, and a malformed or doubled list
line; `measure`, over a stub `cargo` that writes each target's lcov, writes
the record, fails naming a target that runs a floored line, and refuses a
report that never names a floored file (unread, never zero); and the planted
call of #401, the real report of `test header_source` made to call
`spend_gate::allow_pending`, refused by the entry's name, while the same
report at zero counts passes. Each case
copies the script into a tree of its own under $TMPDIR. Run by the CI job
ratchet: python3 tools/test_no_floored.py
"""

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), ".github", "nightly", "no_floored.py")

NIGHTLY = """jobs:
  mutants-score:
    steps:
      - name: Score per file against the floors
        env:
          MUTANTS_FLOORS: >-
            {"src/a.rs": 90,
             "src/b.rs": 75}
        run: true
"""
LIST = """# the left-out targets
test arch_one
test two  # a comment after the name
bench quiet
"""
FLOORED = ["src/a.rs", "src/b.rs"]


def load(path):
    with open(path) as f:
        return json.load(f)


def zero(kind, name):
    return {"kind": kind, "name": name, "seconds": 1.0, "reach": {f: [] for f in FLOORED}}


def record(targets=None, floored=None):
    return {
        "issue": 401,
        "commit": "0" * 40,
        "floored": FLOORED if floored is None else floored,
        "targets": targets if targets is not None else
        [zero("test", "arch_one"), zero("test", "two"), zero("bench", "quiet")],
    }


# A cargo stub: `cargo llvm-cov ... --output-path P --test|--bench NAME`
# copies $LCOV_DIR/NAME.lcov to P, with ROOT in it replaced by the tree.
CARGO = """#!/usr/bin/env python3
import os, sys
args = sys.argv[1:]
out = args[args.index("--output-path") + 1]
name = args[-1]
with open(os.path.join(os.environ["LCOV_DIR"], name + ".lcov")) as f:
    text = f.read()
with open(out, "w") as f:
    f.write(text.replace("ROOT", os.getcwd()))
"""


def lcov(hits):
    """An lcov naming both floored files and one other; `hits` maps a floored
    file to the lines it ran."""
    parts = []
    for f in FLOORED + ["src/other.rs"]:
        lines = [f"DA:{n},{2 if n in hits.get(f, ()) else 0}" for n in (3, 7, 9)]
        parts.append("\n".join([f"SF:ROOT/{f}", *lines, "end_of_record"]))
    return "\n".join(parts) + "\n"


class Tree(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="no-floored-", dir=os.environ.get("TMPDIR"))
        self.addCleanup(shutil.rmtree, self.root)
        nightly = os.path.join(self.root, ".github", "nightly")
        os.makedirs(nightly)
        os.makedirs(os.path.join(self.root, ".github", "workflows"))
        shutil.copy(SCRIPT, nightly)
        self.write(".github/workflows/nightly.yml", NIGHTLY)
        self.write(".github/nightly/no-floored-targets", LIST)
        self.write("tests/arch_one.rs", "")
        self.write("tests/two/main.rs", "")
        self.write("benches/quiet.rs", "")
        self.write("src/a.rs", "")
        self.write("src/b.rs", "")

    def write(self, rel, text):
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        return path

    def run_script(self, *args, env=None):
        return subprocess.run(
            [sys.executable, "-B", os.path.join(self.root, ".github", "nightly", "no_floored.py"), *args],
            cwd=self.root, capture_output=True, text=True, env=env)

    def check(self, rec):
        path = self.write("record.json", json.dumps(rec))
        return self.run_script("check", path)


class List(Tree):
    def test_list_prints_one_kinds_names(self):
        self.assertEqual(self.run_script("list", "test").stdout, "arch_one two\n")
        self.assertEqual(self.run_script("list", "bench").stdout, "quiet\n")

    def test_a_malformed_or_doubled_line_is_refused_by_its_line(self):
        self.write(".github/nightly/no-floored-targets", LIST + "doc nothing\ntest two\n")
        r = self.run_script("list", "test")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(r.stdout, "")
        self.assertIn("no-floored-targets:5: not `test <name>`", r.stderr)
        self.assertIn("no-floored-targets:6: test two is listed twice", r.stderr)


class Check(Tree):
    def test_every_entry_at_zero_passes(self):
        r = self.check(record())
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("every left-out target runs no line of a floored file", r.stdout)

    def test_a_target_running_a_floored_line_fails_by_its_name_and_line(self):
        targets = record()["targets"]
        targets[1]["reach"]["src/b.rs"] = [12, 40]
        r = self.check(record(targets))
        self.assertEqual(r.returncode, 1)
        self.assertIn("::error::test two runs src/b.rs:12 and 1 more of its lines: it judges a floored file", r.stdout)
        self.assertNotIn("arch_one runs", r.stdout)

    def test_an_entry_the_record_does_not_measure_fails_by_name(self):
        r = self.check(record(record()["targets"][:2]))
        self.assertEqual(r.returncode, 1)
        self.assertIn("bench quiet is listed in .github/nightly/no-floored-targets but the record does not measure it", r.stdout)

    def test_a_record_entry_off_the_list_fails_by_name(self):
        r = self.check(record(record()["targets"] + [zero("test", "gone")]))
        self.assertEqual(r.returncode, 1)
        self.assertIn("test gone is in the record but not in", r.stdout)

    def test_an_entry_naming_no_target_fails_by_name(self):
        os.remove(os.path.join(self.root, "benches", "quiet.rs"))
        r = self.check(record())
        self.assertEqual(r.returncode, 1)
        self.assertIn("bench quiet is listed in .github/nightly/no-floored-targets but is no target", r.stdout)

    def test_a_floored_file_the_record_does_not_measure_fails_by_name(self):
        targets = record()["targets"]
        for t in targets:
            del t["reach"]["src/b.rs"]
        r = self.check(record(targets, ["src/a.rs"]))
        self.assertEqual(r.returncode, 1)
        self.assertIn("the record does not measure the floored file src/b.rs", r.stdout)


class Stubbed(Tree):
    def measure(self, lcovs):
        bindir = os.path.join(self.root, "bin")
        os.makedirs(bindir)
        cargo = self.write("bin/cargo", CARGO)
        os.chmod(cargo, os.stat(cargo).st_mode | stat.S_IEXEC)
        lcov_dir = os.path.join(self.root, "lcovs")
        os.makedirs(lcov_dir)
        for name, text in lcovs.items():
            self.write(f"lcovs/{name}.lcov", text)
        env = dict(os.environ, PATH=bindir + os.pathsep + os.environ["PATH"], LCOV_DIR=lcov_dir)
        out = os.path.join(self.root, "out.json")
        return self.run_script("measure", out, "--lcov-dir", os.path.join(self.root, "reports"), env=env), out


class Measure(Stubbed):
    def test_zero_reach_writes_a_record_the_check_passes(self):
        r, out = self.measure({"arch_one": lcov({}), "two": lcov({}), "quiet": lcov({})})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        rec = load(out)
        self.assertEqual([(t["kind"], t["name"]) for t in rec["targets"]],
                         [("test", "arch_one"), ("test", "two"), ("bench", "quiet")])
        self.assertEqual(rec["floored"], FLOORED)
        self.assertEqual(self.check(rec).returncode, 0)

    def test_a_target_reaching_a_floored_line_fails_by_its_name(self):
        r, out = self.measure({"arch_one": lcov({"src/a.rs": (7, 9)}), "two": lcov({}), "quiet": lcov({})})
        self.assertEqual(r.returncode, 1)
        self.assertIn("::error::test arch_one runs src/a.rs:7 and 1 more of its lines", r.stdout)
        self.assertEqual(load(out)["targets"][0]["reach"]["src/a.rs"], [7, 9])

    def test_a_report_naming_no_floored_file_is_unread_never_zero(self):
        r, _ = self.measure({"arch_one": "SF:ROOT/src/other.rs\nDA:1,1\nend_of_record\n"})
        self.assertEqual(r.returncode, 1)
        self.assertIn("::error::test arch_one: the coverage report names no src/a.rs, src/b.rs; its reach is unread, never zero", r.stdout)


# The planted call of #401: on a scratch branch, `test header_source` was made
# to call `bsv_wallet_cli::spend_gate::allow_pending` once, and the job's
# `measure` (cargo-llvm-cov 0.6.14) refused it by name. PLANTED is that run's
# report for the floored file, cut to the plant's function and its lines with
# their counts as recorded (LF and FNF are the whole file's); UNPLANTED is the
# same lines at the counts of the tree without the plant.
PLANTED = """SF:ROOT/src/spend_gate.rs
FN:343,_RNvNtCs5b2QZ1eff7g_14bsv_wallet_cli10spend_gate13allow_pending
FNDA:1,_RNvNtCs5b2QZ1eff7g_14bsv_wallet_cli10spend_gate13allow_pending
FNF:136
FNH:1
DA:343,1
DA:344,1
DA:345,1
DA:350,0
BRF:0
BRH:0
LF:1176
LH:3
end_of_record
"""
UNPLANTED = (PLANTED.replace("FNDA:1,", "FNDA:0,").replace("FNH:1", "FNH:0")
             .replace(",1\nDA", ",0\nDA").replace("DA:345,1", "DA:345,0").replace("LH:3", "LH:0"))


class Planted(Stubbed):
    def setUp(self):
        super().setUp()
        self.write(".github/workflows/nightly.yml",
                   NIGHTLY.replace('{"src/a.rs": 90,\n             "src/b.rs": 75}', '{"src/spend_gate.rs": 70}'))
        self.write(".github/nightly/no-floored-targets", "test header_source\n")
        self.write("tests/header_source.rs", "")

    def test_the_planted_call_is_refused_by_the_entrys_name(self):
        r, out = self.measure({"header_source": PLANTED})
        self.assertEqual(r.returncode, 1)
        self.assertIn(
            "::error::test header_source runs src/spend_gate.rs:343 and 2 more of its lines: it judges "
            "a floored file, so it joins the oracle; take it out of .github/nightly/no-floored-targets\n"
            "measure: 1 fault(s); the left-out targets' zero reach does not hold\n", r.stdout)
        self.assertEqual(load(out)["targets"][0]["reach"]["src/spend_gate.rs"], [343, 344, 345])
        self.assertEqual(self.check(load(out)).returncode, 1)

    def test_the_same_report_without_the_plant_passes(self):
        self.assertNotIn(",1\n", UNPLANTED)
        r, out = self.measure({"header_source": UNPLANTED})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("measure: every left-out target runs no line of a floored file", r.stdout)
        self.assertEqual(load(out)["targets"][0]["reach"]["src/spend_gate.rs"], [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
