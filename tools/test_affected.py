#!/usr/bin/env python3
"""The planted cases of CI's test selection (#473), .github/ci/affected.py,
on captured diffs: a one-file source change (a floored file) runs the unit
tests and every target but those the reach record measures running none of
it, the targets that read src/ as text kept; a test-file change runs its
target alone, a shared test module the targets that declare it; a lockfile,
a manifest, a .github or a tools change, and a file without a reach record
(code the lib compiles that no record measures, a fixture read at run time)
run everything; test-only code runs the unit tests; a bench runs no test;
deny runs on the paths it reads; every event but pull_request runs the whole
suite; the outputs CI reads; and the same rules on this repository's own
tree. Each planted case writes a tree under $TMPDIR. Run by the CI job lint:
python3 tools/test_affected.py
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
REPO = os.path.dirname(HERE)
SCRIPT = os.path.join(REPO, ".github", "ci", "affected.py")

NIGHTLY = """jobs:
  mutants-score:
    steps:
      - name: Score per file against the floors
        env:
          MUTANTS_FLOORS: >-
            {"src/a.rs": 90,
             "src/b.rs": 80}
        run: true
"""
NONE = {"src/a.rs": [], "src/b.rs": []}
REACH = {"issue": 401, "commit": "a" * 40, "floored": ["src/a.rs", "src/b.rs"], "targets": [
    {"kind": "test", "name": "plain", "seconds": 1, "reach": NONE},
    {"kind": "test", "name": "scanner", "seconds": 1, "reach": NONE},
    {"kind": "test", "name": "measured_b", "seconds": 1, "reach": dict(NONE, **{"src/b.rs": [3]})},
    {"kind": "bench", "name": "bb", "seconds": 1, "reach": NONE},
]}
TEST = "#[test]\nfn t() {}\n"
TREE = {
    ".github/workflows/nightly.yml": NIGHTLY,
    ".github/nightly/no-floored-reach.json": json.dumps(REACH),
    "Cargo.toml": "[package]\nname = \"bsv-wallet-cli\"\n",
    "Cargo.lock": "",
    "src/lib.rs": "pub mod a;\npub mod b;\npub mod other;\n#[cfg(test)]\nmod support;\n",
    "src/main.rs": "fn main() {}\n",
    "src/a.rs": "pub fn f() {}\n",
    "src/b.rs": "pub fn f() {}\n",
    "src/other.rs": "pub fn f() {}\n",
    "src/support.rs": "#[test]\nfn t() { crate::a::f(); }\n",
    "src/notes.txt": "a note no source reads\n",
    "tests/plain.rs": TEST,
    # A doc comment naming src/ is not a read; the string is.
    "tests/scanner.rs": "//! reads \"src/a.rs\" as text\nconst ROOT: &str = \"src\";\n" + TEST,
    "tests/measured_b.rs": TEST,
    "tests/uses_shared.rs": "mod shared;\n" + TEST,
    "tests/uses_shared2.rs": "mod shared;\n" + TEST,
    "tests/shared/mod.rs": "mod deeper;\npub fn h() {}\n",
    "tests/shared/deeper.rs": "pub fn g() {}\n",
    "tests/dir_target/main.rs": "mod helper;\n" + TEST,
    "tests/dir_target/helper.rs": "pub fn h() {}\n",
    "tests/data/x.json": "{}\n",
    "benches/bb.rs": "fn main() {}\n",
    "tools/ratchet": "#!/bin/sh\n",
    "docs/ratchets.json": "{}\n",
}
EVERY_BUT = ["dir_target", "measured_b", "plain", "scanner", "uses_shared", "uses_shared2"]


class Planted(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="affected-", dir=os.environ.get("TMPDIR"))
        for path, text in TREE.items():
            p = os.path.join(self.root, path)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "w") as f:
                f.write(text)

    def tearDown(self):
        shutil.rmtree(self.root)

    def run_it(self, paths, event="pull_request", root=None):
        listed = os.path.join(self.root, "paths.txt")
        with open(listed, "w") as f:
            f.write("".join(p + "\n" for p in paths))
        out = os.path.join(self.root, "github-output")
        open(out, "w").close()
        r = subprocess.run([sys.executable, "-B", SCRIPT, "--event", event, "--paths", listed,
                            "--github-output", out, "--root", root or self.root],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(out) as f:
            got = dict(line.rstrip("\n").split("=", 1) for line in f if line.strip())
        got["stdout"] = r.stdout
        return got

    def selected(self, got):
        words = got["test_args"].split()
        return [words[i + 1] for i, w in enumerate(words) if w == "--test"]

    def assert_all(self, got):
        self.assertEqual(got["mode"], "all", got["stdout"])
        self.assertEqual(got["test_args"], "--workspace")
        self.assertEqual((got["test_run"], got["wire"]), ("true", "true"))

    # A one-file source change: a floored file.
    def test_floored_file_runs_unit_tests_and_targets_but_the_measured_none(self):
        got = self.run_it(["src/a.rs"])
        self.assertEqual(got["mode"], "affected")
        self.assertTrue(got["test_args"].startswith("--workspace --lib --bins "))
        # plain and measured_b ran no line of src/a.rs; scanner reads src/ as text.
        self.assertEqual(self.selected(got), ["dir_target", "scanner", "uses_shared", "uses_shared2"])
        self.assertEqual((got["test_run"], got["wire"], got["deny"]), ("true", "false", "false"))
        self.assertIn("selection: affected (the unit tests, dir_target, scanner", got["line"])

    def test_floored_file_a_target_reaches_runs_it(self):
        got = self.run_it(["src/b.rs"])
        self.assertEqual(self.selected(got), ["dir_target", "measured_b", "scanner", "uses_shared", "uses_shared2"])

    # A test-file change.
    def test_test_file_runs_itself_alone(self):
        got = self.run_it(["tests/plain.rs"])
        self.assertEqual((got["mode"], got["test_args"]), ("affected", "--workspace --test plain"))
        self.assertEqual(got["line"], "selection: affected (plain)")

    def test_a_directory_target_file_runs_that_target(self):
        got = self.run_it(["tests/dir_target/helper.rs"])
        self.assertEqual(got["test_args"], "--workspace --test dir_target")

    def test_a_shared_test_module_runs_the_targets_that_declare_it(self):
        got = self.run_it(["tests/shared/deeper.rs"])
        self.assertEqual(self.selected(got), ["uses_shared", "uses_shared2"])
        self.assertNotIn("--lib", got["test_args"])

    # A lockfile change, a manifest, .github and tools.
    def test_lockfile_runs_everything_and_deny(self):
        got = self.run_it(["Cargo.lock"])
        self.assert_all(got)
        self.assertEqual(got["deny"], "true")

    def test_manifest_runs_everything_and_deny(self):
        got = self.run_it(["tests/plain.rs", "Cargo.toml"])
        self.assert_all(got)
        self.assertEqual(got["deny"], "true")

    def test_github_change_runs_everything(self):
        got = self.run_it([".github/workflows/nightly.yml"])
        self.assert_all(got)
        self.assertEqual(got["deny"], "false")
        self.assertEqual(self.run_it([".github/workflows/ci.yml"])["deny"], "true")

    def test_tools_change_runs_everything(self):
        self.assert_all(self.run_it(["tools/ratchet"]))

    # A file without a reach record.
    def test_code_no_record_measures_runs_everything(self):
        got = self.run_it(["src/other.rs"])
        self.assert_all(got)
        self.assertIn("the record measures no target's reach into it", got["stdout"])

    def test_a_fixture_read_at_run_time_runs_everything(self):
        self.assert_all(self.run_it(["tests/data/x.json"]))

    def test_a_record_and_a_stray_note_run_everything(self):
        self.assert_all(self.run_it(["docs/ratchets.json"]))
        self.assert_all(self.run_it(["src/notes.txt"]))

    # The narrower kinds.
    def test_test_only_code_runs_the_unit_tests_and_the_text_readers(self):
        got = self.run_it(["src/support.rs"])
        self.assertEqual(got["test_args"], "--workspace --lib --bins --test scanner")

    def test_a_bench_runs_no_test(self):
        got = self.run_it(["benches/bb.rs"])
        self.assertEqual((got["mode"], got["test_run"], got["wire"]), ("affected", "false", "false"))
        self.assertEqual(got["line"], "selection: affected (no test target)")

    def test_the_union_of_two_paths(self):
        got = self.run_it(["tests/plain.rs", "tests/shared/mod.rs"])
        self.assertEqual(self.selected(got), ["plain", "uses_shared", "uses_shared2"])

    # The events.
    def test_every_other_event_runs_the_whole_suite(self):
        for event in ("merge_group", "push", "workflow_call"):
            got = self.run_it(["tests/plain.rs"], event=event)
            self.assert_all(got)
            self.assertEqual(got["deny"], "true", event)
            self.assertIn(f"{event}: the whole suite", got["line"])

    def test_no_path_runs_the_whole_suite(self):
        self.assert_all(self.run_it([]))


class ThisRepository(unittest.TestCase):
    """The rules on this tree: the left-out targets that read src/ as text stay."""

    def run_it(self, path):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as d:
            listed = os.path.join(d, "paths")
            with open(listed, "w") as f:
                f.write(path + "\n")
            out = os.path.join(d, "out")
            r = subprocess.run([sys.executable, "-B", SCRIPT, "--event", "pull_request", "--paths", listed,
                                "--github-output", out], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            with open(out) as f:
                return dict(line.rstrip("\n").split("=", 1) for line in f if line.strip())

    def test_a_test_file(self):
        self.assertEqual(self.run_it("tests/json_refusal.rs")["test_args"], "--workspace --test json_refusal")

    def test_the_wire_support_module(self):
        got = self.run_it("tests/wire_support/mod.rs")
        self.assertEqual((got["test_args"], got["wire"]), ("--workspace --test wire --test wire_consumers", "true"))

    def test_a_floored_file(self):
        got = self.run_it("src/spend_gate.rs")
        words = got["test_args"].split()
        for kept in ("arch_no_unwrap", "rule_28_sites", "e2e_hermetic", "wire", "integration"):
            self.assertIn(kept, words)
        for left in ("brc100", "header_source", "signing_digest_vector"):
            self.assertNotIn(left, words)

    def test_the_lockfile(self):
        got = self.run_it("Cargo.lock")
        self.assertEqual((got["mode"], got["deny"]), ("all", "true"))


if __name__ == "__main__":
    unittest.main()
