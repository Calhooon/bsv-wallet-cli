#!/usr/bin/env python3
"""The planted cases of the Nightly's plan (#462), .github/nightly/changed_set.py,
on captured inputs: a source change mutates that floored file alone, in the
shards its mutants need, and carries the rest; a test change reaches every
floored file when the reach record does not measure the target, the files it
ran a line of when it does, and none for a left-out target; a lockfile or a
workspace manifest change brings in the whole set; no change plans no shard
(the night carries everything); a complete night mutates every file in
MUTANTS_SHARDS shards; a record without a mode is refused; and the git diff
against the record's last complete run, on a repository of its own. Each case
writes a tree under $TMPDIR. Run by the CI job ratchet:
python3 tools/test_changed_set.py
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
SCRIPT = os.path.join(os.path.dirname(HERE), ".github", "nightly", "changed_set.py")

NIGHTLY = """jobs:
  mutants-score:
    steps:
      - name: Score per file against the floors
        env:
          MUTANTS_FLOORS: >-
            {"src/a.rs": 90,
             "src/b.rs": 75,
             "src/c.rs": 80}
        run: true
"""
RUN = "https://github.com/Calhooon/bsv-wallet-cli/actions/runs/1000"
SHA = "a" * 40


def counts(n):
    return {"score": 50.0, "caught": n // 2, "missed": n - n // 2, "timeout": 0, "unviable": 0,
            "source": "night", "run": RUN, "sha": SHA}


RECORD = {"schema": 1, "mode": "complete", "commit": SHA, "run": RUN, "date": "2026-11-02",
          "files": {"src/a.rs": counts(40), "src/b.rs": counts(300), "src/c.rs": counts(20)}}
NONE = {"src/a.rs": [], "src/b.rs": [], "src/c.rs": []}
REACH = {"issue": 401, "commit": SHA, "floored": ["src/a.rs", "src/b.rs", "src/c.rs"], "targets": [
    {"kind": "test", "name": "arch_rule", "seconds": 1, "reach": NONE},
    {"kind": "test", "name": "measured", "seconds": 1, "reach": dict(NONE, **{"src/c.rs": [12, 13]})},
]}


class Tree:
    def __init__(self):
        self.root = tempfile.mkdtemp(prefix="changed-set-case.", dir=os.environ.get("TMPDIR"))
        self.write(".github/workflows/nightly.yml", NIGHTLY)
        self.write(".github/nightly/no-floored-targets", "test arch_rule\n")
        self.write(".github/nightly/no-floored-reach.json", json.dumps(REACH))
        self.write("docs/scoreboard/mutants.json", json.dumps(RECORD))
        for name in ("src/a.rs", "src/b.rs", "src/c.rs", "src/other.rs", "tests/arch_rule.rs",
                     "tests/measured.rs", "tests/oracle.rs", "tests/integration/main.rs",
                     "benches/gate.rs", "Cargo.toml", "Cargo.lock"):
            self.write(name, "// x\n")

    def write(self, name, text):
        p = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def plan(self, *paths, mode="incremental", git=False):
        out = os.path.join(self.root, "plan.json")
        args = [sys.executable, "-B", SCRIPT, "--root", self.root, "--mode", mode, "--out", out]
        if not git:
            self.write("changed.txt", "".join(p + "\n" for p in paths))
            args += ["--paths", os.path.join(self.root, "changed.txt"), "--head", "f" * 40]
        r = subprocess.run(args, capture_output=True, text=True)
        text = (r.stdout + r.stderr).strip()
        if r.returncode != 0:
            return r.returncode, text, None
        with open(out) as f:
            return r.returncode, text, json.load(f)

    def git(self, *args):
        return subprocess.run(["git", "-C", self.root, *args], capture_output=True, text=True, check=True).stdout.strip()

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


class Case(unittest.TestCase):
    def setUp(self):
        self.tree = Tree()

    def tearDown(self):
        self.tree.cleanup()

    def planned(self, *paths, **kw):
        code, out, p = self.tree.plan(*paths, **kw)
        self.assertEqual(code, 0, out)
        return p, out

    def show(self, p):
        print(f"\n    files {p['files']}, shards {p['shards']}, carried {p['carried']}", end=" ", file=sys.stderr)

    def test_a_source_change_mutates_that_file_alone(self):
        p, out = self.planned("src/a.rs", "src/other.rs", "docs/x.md")
        self.assertEqual(p["files"], ["src/a.rs"])
        self.assertEqual(p["carried"], ["src/b.rs", "src/c.rs"])
        # ceil(40 / 19) is 3; the six runners a night holds come first.
        self.assertEqual((p["mutants"], p["shards"], p["matrix"]), (40, 6, [0, 1, 2, 3, 4, 5]))
        self.assertFalse(p["whole"])
        self.assertIn("Carried from the last complete run: `src/b.rs`, `src/c.rs`.", out)
        self.show(p)

    def test_a_test_change_the_record_does_not_measure_reaches_every_file(self):
        for path in ("tests/oracle.rs", "tests/integration/auth.rs", "tests/wire/001.json", "benches/gate.rs",
                     "fuzz/corpus/beef_reader/seed", "fuzz/src/props/x.rs"):
            p, _ = self.planned(path)
            self.assertEqual(p["files"], ["src/a.rs", "src/b.rs", "src/c.rs"], path)
            self.assertEqual(p["shards"], 50, path)
        self.show(p)

    def test_a_measured_test_change_reaches_the_files_it_ran(self):
        p, _ = self.planned("tests/measured.rs")
        self.assertEqual((p["files"], p["shards"]), (["src/c.rs"], 6))
        self.show(p)

    def test_a_left_out_test_change_reaches_nothing(self):
        p, out = self.planned("tests/arch_rule.rs")
        self.assertEqual((p["files"], p["shards"], p["matrix"]), ([], 0, []))
        self.assertIn("no floored file (the reach record, #401)", json.dumps(p["paths"]))

    def test_a_lockfile_change_brings_in_the_whole_set(self):
        for path in ("Cargo.lock", "Cargo.toml", "mcp/Cargo.toml", ".github/workflows/nightly.yml",
                     ".github/nightly/no-floored-targets"):
            p, _ = self.planned(path)
            self.assertTrue(p["whole"], path)
            self.assertEqual((p["files"], p["shards"]), (["src/a.rs", "src/b.rs", "src/c.rs"], 50), path)
        self.show(p)

    def test_no_change_carries_everything(self):
        p, out = self.planned("README.md", "benches/baseline.json", "fuzz/Cargo.lock", "src/other.rs")
        self.assertEqual((p["files"], p["shards"], p["matrix"]), ([], 0, []))
        self.assertEqual(p["carried"], ["src/a.rs", "src/b.rs", "src/c.rs"])
        self.assertIn("carries every file's figure from the last complete run", out)
        self.show(p)

    def test_a_complete_night_mutates_every_file(self):
        p, _ = self.planned("README.md", mode="complete")
        self.assertEqual((p["files"], p["shards"], p["carried"]), (["src/a.rs", "src/b.rs", "src/c.rs"], 50, []))

    def test_the_selection_leaves_out_the_left_out_list_alone(self):
        p, _ = self.planned("src/a.rs")
        self.assertEqual(p["selection"], "--lib --bins --test measured --test oracle --test integration --bench gate")

    def test_a_record_without_a_mode_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps({k: v for k, v in RECORD.items() if k != "mode"}))
        code, out, _ = self.tree.plan("src/a.rs")
        self.assertEqual(code, 1, out)
        self.assertIn("names no mode", out)

    def test_an_incremental_record_diffs_against_its_base(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(dict(
            RECORD, mode="incremental", commit="b" * 40, run=RUN.replace("1000", "2000"),
            base={"run": RUN, "commit": SHA})))
        p, _ = self.planned("src/b.rs")
        self.assertEqual(p["base"], {"run": RUN, "commit": SHA})

    def test_the_git_diff_against_the_last_complete_run(self):
        t = self.tree
        t.git("init", "-q")
        t.git("-c", "user.name=t", "-c", "user.email=t@t", "add", "-A")
        t.git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base")
        base = t.git("rev-parse", "HEAD")
        t.write("docs/scoreboard/mutants.json", json.dumps(dict(RECORD, commit=base)))
        t.write("src/b.rs", "// changed\n")
        t.git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "head")
        p, _ = self.planned(git=True)
        self.assertEqual((p["base"]["commit"], p["files"]), (base, ["src/b.rs"]))
        self.assertEqual(p["shards"], 16)  # ceil(300 / 19)
        self.show(p)


if __name__ == "__main__":
    unittest.main(verbosity=2)
