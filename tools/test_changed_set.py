#!/usr/bin/env python3
"""The planted cases of the Nightly's plan (#462), .github/nightly/changed_set.py,
on captured inputs: a source change mutates that floored file alone, in the
shards its mutants need, and carries the rest; a test change reaches every
floored file when the reach record does not measure the target, the files it
ran a line of when it does, and none for a left-out target; a lockfile or a
workspace manifest change brings in the whole set; no change plans no shard
(the night carries everything); a file under src/, read by the crate's
module tree (#464's review: a test module reaches the floored files its tests
can run by name, other code the whole set), on a planted crate and on this
repository's own; a complete night mutates every file in MUTANTS_SHARDS
shards; a record without a mode is refused; and the git diff
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


# A crate shaped like the reviewers' cases (#464): floored modules with test
# modules in their module directories (`mod tests;`, a subdirectory, a
# `#[path]`), a module the lib compiles outside its tests, test-only helpers,
# a file include_str! reads, and a file a test target compiles by #[path].
TEST = "    #[test]\n    fn t() {}\n"
CRATE = {
    "src/lib.rs": ("pub mod a;\npub mod b;\npub mod c;\npub mod other;\n"
                   "#[cfg(test)]\nmod support;\n#[cfg(test)]\nmod unused;\n#[cfg(test)]\nmod tracker;\n"),
    "src/main.rs": "mod cli;\nfn main() { cli::run(); bsv_wallet_cli::other::f(); }\n",
    "src/cli.rs": "pub fn run() {}\n",
    "src/a.rs": ("mod from;\npub fn f() { from::g(); }\n#[cfg(test)]\nmod tests;\n"
                 "#[cfg(test)]\n#[path = \"a_extra_tests.rs\"]\nmod extra;\n"),
    "src/a/from.rs": "pub fn g() {}\n",
    "src/a/tests.rs": "use super::*;\nmod carried;\n" + TEST,
    "src/a/tests/carried.rs": "use super::*;\n" + TEST,
    "src/a_extra_tests.rs": "use super::f;\n" + TEST,
    "src/b.rs": "pub fn f() {}\n#[cfg(test)]\nmod tests;\n",
    "src/b/tests.rs": "use crate::{c, support};\n#[test]\nfn t() { c::f(); support::h(); }\n",
    "src/c.rs": "pub fn f() {}\n",
    "src/other.rs": ("// A string is not a name: \"crate::a::f\"\npub fn f() {}\n"
                     "pub const DATA: &str = include_str!(\"data.json\");\n#[cfg(test)]\nmod tests;\n"),
    "src/other/tests.rs": "#[test]\nfn t() { crate::c::f(); }\n",
    "src/data.json": "{}\n",
    "src/NOTES.md": "notes\n",
    "src/support.rs": "pub fn h() {}\n",
    "src/unused.rs": "pub fn u() {}\n",
    "src/tracker.rs": "pub fn k() {}\n",
    "tests/measured.rs": "#[path = \"../src/tracker.rs\"]\nmod tracker;\n",
}


class Tree:
    def __init__(self):
        self.root = tempfile.mkdtemp(prefix="changed-set-case.", dir=os.environ.get("TMPDIR"))
        self.write(".github/workflows/nightly.yml", NIGHTLY)
        self.write(".github/nightly/no-floored-targets", "test arch_rule\n")
        self.write(".github/nightly/no-floored-reach.json", json.dumps(REACH))
        self.write("docs/scoreboard/mutants.json", json.dumps(RECORD))
        for name in ("tests/arch_rule.rs", "tests/oracle.rs", "tests/integration/main.rs",
                     "benches/gate.rs", "Cargo.toml", "Cargo.lock"):
            self.write(name, "// x\n")
        for name, text in CRATE.items():
            self.write(name, text)

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
        p, out = self.planned("src/a.rs", "src/NOTES.md", "docs/x.md")
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
        p, out = self.planned("README.md", "benches/baseline.json", "fuzz/Cargo.lock", "src/NOTES.md")
        self.assertEqual((p["files"], p["shards"], p["matrix"]), ([], 0, []))
        self.assertEqual(p["carried"], ["src/a.rs", "src/b.rs", "src/c.rs"])
        self.assertIn("carries every file's figure from the last complete run", out)
        self.show(p)

    def test_a_test_module_in_a_floored_module_directory_reaches_that_file(self):
        # #464's review: src/spend_gate/tests.rs (`mod tests;`) and
        # src/spend_gate/tests/carried.rs planned no file. A test module, its
        # subdirectory and a `#[path]` test module of a.rs reach a.rs; a.rs
        # names no other floored module, so a.rs alone: 40 mutants, six shards.
        for path in ("src/a/tests.rs", "src/a/tests/carried.rs", "src/a_extra_tests.rs"):
            p, _ = self.planned(path)
            self.assertEqual((p["files"], p["shards"]), (["src/a.rs"], 6), path)
            self.assertIn("test-only code", p["paths"][0]["why"], path)
        self.show(p)

    def test_a_test_module_reaches_every_floored_module_it_names(self):
        # b's tests name crate::c: b and c, 320 mutants, ceil(320 / 19) shards.
        p, _ = self.planned("src/b/tests.rs")
        self.assertEqual((p["files"], p["shards"]), (["src/b.rs", "src/c.rs"], 17))
        # other.rs's #[cfg(test)] module names crate::c: c, 20 mutants, six shards.
        p, _ = self.planned("src/other/tests.rs")
        self.assertEqual((p["files"], p["shards"]), (["src/c.rs"], 6))
        self.show(p)

    def test_a_test_helper_reaches_what_the_tests_that_name_it_reach(self):
        # support.rs names nothing; b's tests name it and reach b and c.
        p, _ = self.planned("src/support.rs")
        self.assertEqual((p["files"], p["shards"]), (["src/b.rs", "src/c.rs"], 17))
        # A test-only module no test names reaches nothing...
        p, _ = self.planned("src/unused.rs")
        self.assertEqual((p["files"], p["shards"]), ([], 0))
        # ...unless a test target compiles it by #[path]: the target's reach.
        p, _ = self.planned("src/tracker.rs")
        self.assertEqual((p["files"], p["shards"]), (["src/c.rs"], 6))
        self.assertIn("compiled by tests/measured.rs (#[path]): test measured", p["paths"][0]["why"])
        self.show(p)

    def test_code_outside_the_tests_brings_in_the_whole_set(self):
        # #464's review: src/spend_gate/from.rs, and src/config.rs, src/main.rs,
        # src/server/mod.rs, src/lib.rs (which hold #[cfg(test)] modules):
        # code the lib or a binary compiles outside its tests may run beside
        # any floored file in a unit test, the conservative choice.
        for path in ("src/a/from.rs", "src/other.rs", "src/lib.rs", "src/main.rs", "src/cli.rs",
                     "src/data.json", "src/stray.rs"):
            p, _ = self.planned(path)
            self.assertEqual((p["files"], p["shards"], p["whole"]), (["src/a.rs", "src/b.rs", "src/c.rs"], 50, True),
                             path)
        self.assertIn("read by src/other.rs", self.planned("src/data.json")[0]["paths"][0]["why"])
        self.show(p)

    def test_a_path_in_a_floored_module_directory_reaches_that_file(self):
        p, _ = self.planned("src/a/vectors.bin", "src/NOTES.md")
        self.assertEqual((p["files"], p["shards"]), (["src/a.rs"], 6))

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


class Repo(unittest.TestCase):
    """The reviewers' rows on this repository's own crate (#464): each planned
    no file at 4443492. Every floored file here names the others (handlers ->
    spend_gate -> tracker_host -> ...), so a test that names one reaches all."""

    def test_the_reviewers_rows_bring_in_the_whole_set(self):
        root = os.path.dirname(HERE)
        sys.path.insert(0, os.path.dirname(SCRIPT))
        import no_floored
        floored = no_floored.floored(__import__("pathlib").Path(root))
        scratch = tempfile.mkdtemp(prefix="changed-set-repo.", dir=os.environ.get("TMPDIR"))
        try:
            for path in ("src/spend_gate/tests.rs", "src/server/handlers/tests.rs", "src/spend_gate/from.rs",
                         "src/spend_gate/tests/carried.rs", "src/config.rs", "src/main.rs", "src/server/mod.rs",
                         "src/lib.rs", "src/config/tests.rs"):
                with open(os.path.join(scratch, "changed.txt"), "w") as f:
                    f.write(path + "\n")
                out = os.path.join(scratch, "plan.json")
                r = subprocess.run([sys.executable, "-B", SCRIPT, "--root", root, "--mode", "incremental",
                                    "--paths", os.path.join(scratch, "changed.txt"), "--head", "f" * 40,
                                    "--out", out], capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                with open(out) as f:
                    p = json.load(f)
                self.assertEqual((p["files"], p["shards"]), (sorted(floored), 50), path)
                print(f"\n    {path}: {len(p['files'])} files, {p['shards']} shards", end=" ", file=sys.stderr)
        finally:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
