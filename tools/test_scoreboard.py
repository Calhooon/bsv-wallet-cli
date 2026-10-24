#!/usr/bin/env python3
"""The planted cases of tools/scoreboard (#77): each record that is present and
malformed exits 1 naming its file and field, each number that falls from the
base's values fails --check --base, and the cases it must let through. Each
case runs the tool on a tree of its own under $TMPDIR (--root). Run by the CI
job ratchet: python3 tools/test_scoreboard.py
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
TOOL = os.path.join(HERE, "scoreboard")

SHA = "abcdef0" + "1" * 33
DOC = "# x\n\n<!-- scoreboard:begin -->\n<!-- scoreboard:end -->\n"
NIGHTLY = """jobs:
  mutants:
    steps:
      - name: score
        env:
          MUTANTS_FLOORS: >-
            {"src/spend_gate.rs": 80,
             "src/explorers.rs": 69}
        run: true
"""
ARCH = '''use arch_support::Rule;

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/a.rs", 2, "a reason"),
    (
        "src/b/c.rs",
        3,
        "a reason with \\"quotes\\" and a (paren)",
    ),
];

const RULE: Rule = Rule {
    name: "A4: unwrap/expect outside test code",
    homes: &[],
    allowed: ALLOWED,
    sites: |l| 0,
};
'''
SCENARIO = """const { x } = require('../lib/x');

module.exports = {
  name: 'S',
  description: 'a scenario',
  async run(walletA, walletB, assert) {
    return { ok: { run: 1 } };
  },
};
"""
MUTANTS = {"schema": 1, "commit": SHA, "run": "https://example.invalid/run/1", "date": "2026-10-23",
           "files": {"src/spend_gate.rs": {"score": 86.6, "caught": 194, "missed": 30,
                                           "timeout": 0, "unviable": 12}}}
BENCH = {"margin": 0.25, "profiles": {"dev-m": {"profile": "dev-m", "gate": True, "runs": [
    {"commit": SHA, "benches": {"gate/1": {"p50_us": 1.0, "p99_us": 2.0, "calls": 9}}}]}}}


class Tree:
    """A tree of the records the tool reads, under $TMPDIR."""

    def __init__(self):
        self.root = tempfile.mkdtemp(prefix="scoreboard-case.", dir=os.environ.get("TMPDIR"))
        self.write("docs/NORTH-STAR.md", DOC)
        self.write("docs/ratchets.json", json.dumps({"commit": SHA, "ratchets": {"missing_docs": 154}}))
        self.write("tests/wire/MANIFEST.json", json.dumps({"fixtures": ["001.json", "002.json"]}))
        self.write(".github/workflows/nightly.yml", NIGHTLY)
        self.write("tests/arch_no_unwrap.rs", ARCH)
        for i in range(3):
            self.write(f"tests/e2e/scenarios/{i:02}-s.js", SCENARIO)
        self.write("tests/e2e/run.js", "const hermetic = require('./lib/hermetic');\n")
        self.write("tests/e2e/lib/hermetic.js", "const enabled = process.env.E2E_HERMETIC === '1';\n")
        for c, n in (("mcp", 2), ("x402-skill", 1)):
            for i in range(n):
                self.write(f"tests/wire_consumers/{c}/{i:02}.json", "{}")

    def write(self, name, text):
        p = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def remove(self, name):
        p = os.path.join(self.root, name)
        shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)

    def run(self, *args):
        r = subprocess.run([sys.executable, "-B", TOOL, "--root", self.root, *args],
                           capture_output=True, text=True)
        return r.returncode, (r.stdout + r.stderr).strip()

    def values(self):
        code, out = self.run("--json")
        assert code == 0, out
        path = os.path.join(self.root, "base-values.json")
        with open(path, "w") as f:
            f.write(out)
        return path

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


def tail(out):
    return out.splitlines()[-1] if out else ""


class Case(unittest.TestCase):
    def setUp(self):
        self.tree = Tree()
        self.addCleanup(self.tree.cleanup)

    def refused(self, needle, *args):
        code, out = self.tree.run(*args)
        self.assertEqual(code, 1, out)
        self.assertIn(needle, out)
        print(f"\n    exit 1: {tail(out)}", end=" ", file=sys.stderr)

    def table(self):
        code, out = self.tree.run()
        self.assertEqual(code, 0, out)
        return out


class PresentRecordsMatchTheirShape(Case):
    """(A) A missing record says so; a present one parses or the tool exits 1."""

    def test_missing_records_say_so(self):
        out = self.table()
        self.assertIn("| bench baseline | no baseline yet |", out)
        self.assertIn("| 69 | unmeasured |", out)

    def test_an_empty_bench_baseline_is_refused(self):
        self.tree.write("benches/baseline.json", "")
        self.refused("benches/baseline.json: (the file): present and empty")

    def test_a_bench_baseline_that_is_not_json_is_refused(self):
        self.tree.write("benches/baseline.json", "{")
        self.refused("benches/baseline.json: (the file): not JSON")

    def test_a_wrong_shaped_bench_baseline_is_refused(self):
        self.tree.write("benches/baseline.json", json.dumps({"profiles": []}))
        self.refused("benches/baseline.json: profiles: must be an object")
        self.tree.write("benches/baseline.json", json.dumps({"runs": []}))
        self.refused("benches/baseline.json: profiles: must be an object")
        self.tree.write("benches/baseline.json", json.dumps({"profiles": {"p": {"runs": []}}}))
        self.refused("benches/baseline.json: profiles.p.runs: must be a list of at least one run")
        bad = json.loads(json.dumps(BENCH))
        bad["profiles"]["dev-m"]["runs"][0]["benches"]["gate/1"].pop("p99_us")
        self.tree.write("benches/baseline.json", json.dumps(bad))
        self.refused("profiles.dev-m.runs[0].benches.gate/1.p99_us: must be a number")

    def test_a_bench_baseline_renders(self):
        self.tree.write("benches/baseline.json", json.dumps(BENCH))
        self.assertIn("| bench baseline `dev-m` | 1 benches, 1 runs |", self.table())

    def test_mutants_with_files_only_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps({"files": {}}))
        self.refused("docs/scoreboard/mutants.json: schema: must be 1")

    def test_mutants_without_a_commit_is_refused(self):
        bad = dict(MUTANTS)
        bad.pop("commit")
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(bad))
        self.refused("docs/scoreboard/mutants.json: commit: must be the full 40-hex sha")

    def test_mutants_with_no_file_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(dict(MUTANTS, files={})))
        self.refused("docs/scoreboard/mutants.json: files: holds no file")

    def test_mutants_with_a_bad_count_is_refused(self):
        bad = json.loads(json.dumps(MUTANTS))
        bad["files"]["src/spend_gate.rs"]["missed"] = "30"
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(bad))
        self.refused("files.src/spend_gate.rs.missed: must be a count")

    def test_mutants_renders_score_and_commit(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        out = self.table()
        self.assertIn("| 80 | 86.6 at abcdef0 |", out)
        self.assertIn("| 69 | unmeasured |", out)

    def test_ratchets_without_a_commit_is_refused(self):
        self.tree.write("docs/ratchets.json", json.dumps({"ratchets": {"x": 1}}))
        self.refused("docs/ratchets.json: commit: must be a full 40-hex sha")

    def test_ratchets_with_a_non_count_is_refused(self):
        self.tree.write("docs/ratchets.json", json.dumps({"commit": SHA, "ratchets": {"x": 1.5}}))
        self.refused("docs/ratchets.json: ratchets.x: must be a count")

    def test_a_wire_manifest_without_a_list_is_refused(self):
        self.tree.write("tests/wire/MANIFEST.json", json.dumps({"fixtures": "001.json"}))
        self.refused("tests/wire/MANIFEST.json: fixtures: must be a list of fixture names")

    def test_floors_that_are_not_json_are_refused(self):
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace('"src/explorers.rs": 69}', "src/explorers.rs: 69}"))
        self.refused(".github/workflows/nightly.yml: MUTANTS_FLOORS: not a JSON object")

    def test_a_floor_above_100_is_refused(self):
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 69}", ": 690}"))
        self.refused("MUTANTS_FLOORS.src/explorers.rs: must be a score from 0 to 100")


class TheArchRowsAreTheAllowlistsOwnData(Case):
    """(C) Each tests/arch_*.rs ALLOWED literal, parsed strictly."""

    def test_the_allowlist_is_counted(self):
        out = self.table()
        self.assertIn("| architecture rules | 1 |", out)
        self.assertIn("| arch rule A4 allowlist | 2 files, 5 sites |  | `tests/arch_no_unwrap.rs` ALLOWED |", out)

    def test_a_doc_says_nothing(self):
        self.tree.write("docs/ARCH-RULES.md", "| Rule | Allowlist today |\n|---|---|\n")
        self.assertIn("| arch rule A4 allowlist | 2 files, 5 sites |", self.table())

    def test_a_comment_in_the_literal_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('    ("src/a.rs"', '    // gone\n    ("src/a.rs"'))
        self.refused("tests/arch_no_unwrap.rs: ALLOWED entry 1: not a (\"file\", count, \"reason\") tuple")

    def test_a_count_by_name_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/a.rs", 2,', '"src/a.rs", TWO,'))
        self.refused("tests/arch_no_unwrap.rs: ALLOWED entry 1: not a")

    def test_a_raw_string_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('("src/a.rs", 2, "a reason")', '("src/a.rs", 2, r"a reason")'))
        self.refused('tests/arch_no_unwrap.rs: ALLOWED entry 1: a raw string r"..", r#".."# is not read')

    def test_a_raw_file_string_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/b/c.rs"', 'r#"src/b/c.rs"#'))
        self.refused('tests/arch_no_unwrap.rs: ALLOWED entry 2: a raw string r"..", r#".."# is not read')

    def test_an_entry_at_zero_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/a.rs", 2,', '"src/a.rs", 0,'))
        self.refused("ALLOWED entry 1: src/a.rs counts 0")

    def test_a_file_named_twice_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/b/c.rs"', '"src/a.rs"'))
        self.refused("tests/arch_no_unwrap.rs: ALLOWED: names a file twice")

    def test_another_shape_of_literal_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace("&[(&str, usize, &str)]", "&[Entry]"))
        self.refused("tests/arch_no_unwrap.rs: ALLOWED: must hold")

    def test_a_rule_file_without_its_rule_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('name: "A4: ', 'name: "'))
        self.refused("tests/arch_no_unwrap.rs: RULE: must hold one")

    def test_two_files_of_one_rule_are_refused(self):
        self.tree.write("tests/arch_other.rs", ARCH)
        self.refused("RULE: A4 is also tests/arch_no_unwrap.rs's rule")


# The entry of beta's tests/arch_explorer_sites.rs (PR 111) whose line continuation,
# a backslash before a newline, a JSON reader refused with a traceback; copied verbatim.
CRASHED = r'''    (
        "src/services_env.rs",
        33,
        "the explorers' env names (BITAILS_API_KEY, BANANABLOCKS_BASE) and the break-glass warning (14); \
         the bases' configuration (#74, PR 131), 19: the toolbox's bitails and whatsonchain default-URL \
         imports (4), BITAILS_BASE_ENV and its \"BITAILS_BASE\" (2), ExplorerBases' bitails field, its \
         chain defaults and its overrides (13); no request is made here",
    ),
'''
CRASHED_REASON = CRASHED[CRASHED.index('"the explorers'):CRASHED.rindex('",') + 1]


def scoreboard():
    """tools/scoreboard as a module, for its decoder's values."""
    import importlib.machinery
    import importlib.util
    loader = importlib.machinery.SourceFileLoader("scoreboard", TOOL)
    mod = importlib.util.module_from_spec(importlib.util.spec_from_loader("scoreboard", loader))
    loader.exec_module(mod)
    return mod


class TheStringsAreRustLiterals(Case):
    """(C) A file and a reason are read as rustc reads a string literal, never as JSON,
    and a literal it cannot read is a named fault, never a traceback."""

    def with_reason(self, reason):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"a reason"', reason, 1))

    def decoded(self, lit):
        return scoreboard().rust_str(lit, "f", "e")

    def test_a_reason_with_an_escaped_apostrophe_is_read(self):
        self.with_reason(r'"the rung\'s name"')
        self.assertIn("| arch rule A4 allowlist | 2 files, 5 sites |", self.table())
        self.assertEqual(self.decoded(r'"the rung\'s name"'), "the rung's name")

    def test_a_reason_with_an_escaped_quote_is_read(self):
        self.with_reason(r'"its \"BITAILS_BASE\" (2)"')
        self.assertIn("| arch rule A4 allowlist | 2 files, 5 sites |", self.table())
        self.assertEqual(self.decoded(r'"its \"BITAILS_BASE\" (2)"'), 'its "BITAILS_BASE" (2)')

    def test_a_reason_with_a_unicode_escape_is_read(self):
        self.with_reason(r'"a \u{2014} b"')
        self.assertIn("| arch rule A4 allowlist | 2 files, 5 sites |", self.table())
        self.assertEqual(self.decoded(r'"a \u{2014} b \u{1_F600}"'), f"a {chr(0x2014)} b {chr(0x1F600)}")

    def test_the_other_escapes_are_read(self):
        self.assertEqual(self.decoded(r'"\n\r\t\\\0\x41\x7f"'), "\n\r\t\\\0A\x7f")

    def test_the_literal_that_crashed_is_read(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace("\n];", "\n" + CRASHED + "];", 1))
        self.assertIn("| arch rule A4 allowlist | 3 files, 38 sites |", self.table())
        self.assertEqual(self.decoded(CRASHED_REASON),
                         "the explorers' env names (BITAILS_API_KEY, BANANABLOCKS_BASE) and the break-glass"
                         " warning (14); the bases' configuration (#74, PR 131), 19: the toolbox's bitails and"
                         " whatsonchain default-URL imports (4), BITAILS_BASE_ENV and its \"BITAILS_BASE\" (2),"
                         " ExplorerBases' bitails field, its chain defaults and its overrides (13); no request"
                         " is made here")

    def test_a_file_string_with_an_escape_is_refused(self):
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/a.rs"', r'"src/\x61.rs"'))
        self.refused(r'tests/arch_no_unwrap.rs: ALLOWED entry 1: the file "src/\x61.rs" carries an escape;'
                     ' a src/ path never does')

    def test_a_malformed_escape_is_a_named_fault(self):
        for lit, why in ((r'"a \q b"', r"\q is not a Rust escape"),
                         (r'"a \x80"', r"\x80 is not a Rust escape"),
                         (r'"a \u{D800}"', r"\u{D800} is not a Rust escape"),
                         (r'"a \u{1234567}"', r"\u{1234567} is not a Rust escape"),
                         (r'"a \u00e9"', r"\u without braces is not a Rust escape")):
            with self.subTest(lit=lit):
                self.with_reason(lit)
                code, out = self.tree.run()
                self.assertEqual(code, 1, out)
                self.assertTrue(out.startswith(
                    "scoreboard: tests/arch_no_unwrap.rs: ALLOWED entry 1 (src/a.rs's reason): " + why), out)
                self.assertNotIn("Traceback", out)
                print(f"\n    exit 1: {tail(out)}", end=" ", file=sys.stderr)


class NoNumberFallsFromTheBase(Case):
    """(B) --check --base refuses a fall in a floor or a count, a rise in an allowlist."""

    def checked(self, base):
        code, out = self.tree.run("--render")
        self.assertEqual(code, 0, out)
        return self.tree.run("--check", "--base", base)

    def refused_against(self, base, needle):
        code, out = self.checked(base)
        self.assertEqual(code, 1, out)
        self.assertIn(needle, out)
        print(f"\n    exit 1: {[l for l in out.splitlines() if needle in l][0]}", end=" ", file=sys.stderr)

    def test_the_same_tree_passes(self):
        code, out = self.checked(self.tree.values())
        self.assertEqual(code, 0, out)
        self.assertIn("no number moved the wrong way from the base", out)

    def test_a_raised_floor_and_a_shrunk_allowlist_pass(self):
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 69}", ": 80}"))
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/a.rs", 2,', '"src/a.rs", 1,'))
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)

    def test_a_lowered_floor_is_refused(self):
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 80,", ": 70,"))
        self.refused_against(base, "floor:src/spend_gate.rs: 80 at the base, 70 in the tree; it may not fall")

    # The floors' rule (#76): a floor may fall only from a base that carries no
    # measurement for its file to a tree whose record carries one.

    def lower_spend_gate(self, score, floor=75):
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 80,", f": {floor},"))
        if score is not None:
            mutants = json.loads(json.dumps(MUTANTS))
            mutants["files"]["src/spend_gate.rs"]["score"] = score
            self.tree.write("docs/scoreboard/mutants.json", json.dumps(mutants))

    def test_a_provisional_floor_its_first_measurement_lowers_passes(self):
        base = self.tree.values()
        self.lower_spend_gate(75.4)
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        notice = ("scoreboard: notice: floor:src/spend_gate.rs: 80 at the base (provisional, unmeasured), "
                  "75 in the tree (measured)")
        self.assertIn(notice, out)
        print(f"\n    exit 0: {[l for l in out.splitlines() if 'notice' in l][0]}", end=" ", file=sys.stderr)

    def test_a_measured_floor_that_falls_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        base = self.tree.values()
        self.lower_spend_gate(75.4)
        self.refused_against(base, "floor:src/spend_gate.rs: 80 at the base, 75 in the tree; a measured floor may not fall")

    def test_a_provisional_floor_lowered_without_a_measurement_is_refused(self):
        base = self.tree.values()
        self.lower_spend_gate(None)
        self.refused_against(base, "floor:src/spend_gate.rs: 80 at the base, 75 in the tree; it may not fall "
                                   "(a provisional floor may not be lowered without a night's measurement)")

    def test_a_measured_floor_that_rises_passes(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 80,", ": 85,"))
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        self.assertIn("no number moved the wrong way from the base", out)
        print(f"\n    exit 0: {tail(out)}", end=" ", file=sys.stderr)

    def test_a_measurement_that_vanishes_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        base = self.tree.values()
        self.tree.remove("docs/scoreboard/mutants.json")
        self.refused_against(base, "floor:src/spend_gate.rs:measured: measured at the base, unmeasured in the tree")

    def test_a_base_older_than_the_flag_reads_as_unmeasured(self):
        base = self.tree.values()
        with open(base) as f:
            vals = json.load(f)
        vals["values"] = {k: v for k, v in vals["values"].items() if not k.endswith(":measured")}
        with open(base, "w") as f:
            json.dump(vals, f)
        self.lower_spend_gate(75.4)
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        self.assertIn("notice: floor:src/spend_gate.rs: 80 at the base (provisional, unmeasured)", out)

    def test_a_base_missing_one_flag_is_refused(self):
        base = self.tree.values()
        with open(base) as f:
            vals = json.load(f)
        del vals["values"]["floor:src/spend_gate.rs:measured"]
        with open(base, "w") as f:
            json.dump(vals, f)
        self.refused_against(base, "values.floor:src/spend_gate.rs:measured: missing")

    # The first measurement sets the floor at the measured score rounded down,
    # never any lower; a floor never rises above it.

    def test_a_first_measurement_with_the_floor_at_10_is_refused(self):
        base = self.tree.values()
        self.lower_spend_gate(75.5, floor=10)
        self.refused_against(base, "floor:src/spend_gate.rs: measured 75.5 in the tree's record, floor 10 in the tree; "
                                   "a provisional floor takes the measured score rounded down: 75, not 10")

    def test_a_first_measurement_with_the_floor_at_75_passes(self):
        base = self.tree.values()
        self.lower_spend_gate(75.5, floor=75)
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        self.assertIn("notice: floor:src/spend_gate.rs: 80 at the base (provisional, unmeasured), 75 in the tree", out)
        print(f"\n    exit 0: {tail(out)}", end=" ", file=sys.stderr)

    def test_a_first_measurement_with_the_floor_at_76_is_refused(self):
        base = self.tree.values()
        self.lower_spend_gate(75.5, floor=76)
        self.refused_against(base, "floor:src/spend_gate.rs: measured 75.5 in the tree's record, floor 76 in the tree; "
                                   "a provisional floor takes the measured score rounded down: 75, not 76")

    def test_a_measured_floor_raised_to_its_score_rounded_down_passes(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 80,", ": 86,"))
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        self.assertIn("no number moved the wrong way from the base", out)
        print(f"\n    exit 0: {tail(out)}", end=" ", file=sys.stderr)

    def test_a_measured_floor_raised_above_its_score_is_refused(self):
        self.tree.write("docs/scoreboard/mutants.json", json.dumps(MUTANTS))
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", NIGHTLY.replace(": 80,", ": 87,"))
        self.refused_against(base, "floor:src/spend_gate.rs: measured 86.6 in the tree's record, floor 87 in the tree; "
                                   "a floor is never above the measured score rounded down: 86, not 87")

    # A floor left unchanged above a new, lower measurement is refused too.

    def test_a_provisional_floor_left_above_its_first_measurement_is_refused(self):
        base = self.tree.values()
        self.lower_spend_gate(75.5, floor=80)
        self.refused_against(base, "floor:src/spend_gate.rs: measured 75.5 in the tree's record, floor 80 in the tree; "
                                   "a floor is never above the measured score rounded down: 75, not 80")

    def test_a_measured_floor_left_above_a_later_lower_record_is_refused(self):
        self.lower_spend_gate(75.5, floor=75)
        base = self.tree.values()
        self.lower_spend_gate(70.0, floor=75)
        self.refused_against(base, "floor:src/spend_gate.rs: measured 70 in the tree's record, floor 75 in the tree; "
                                   "a floor is never above the measured score rounded down: 70, not 75")

    def test_the_real_unchanged_floors_under_the_nights_record_pass(self):
        # This PR's tip at base and tree: spend_gate 80 under 87.1, explorers 69
        # under 74.2, every floor at or under its score rounded down.
        with open(os.path.join(REPO, ".github/workflows/nightly.yml")) as f:
            self.tree.write(".github/workflows/nightly.yml", f.read())
        with open(os.path.join(REPO, "docs/scoreboard/mutants.json")) as f:
            mutants = f.read()
        scores = {k: v["score"] for k, v in json.loads(mutants)["files"].items()}
        self.assertEqual((scores["src/spend_gate.rs"], scores["src/explorers.rs"]), (87.1, 74.2))
        self.tree.write("docs/scoreboard/mutants.json", mutants)
        base = self.tree.values()
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        self.assertIn("no number moved the wrong way from the base", out)
        print(f"\n    exit 0: {tail(out)}", end=" ", file=sys.stderr)

    def test_the_real_floors_against_the_nights_record_pass(self):
        # This PR's three provisional floors (80 at the base, before any night)
        # against the first full night's record and the workflow's floors.
        with open(os.path.join(REPO, ".github/workflows/nightly.yml")) as f:
            nightly = f.read()
        with open(os.path.join(REPO, "docs/scoreboard/mutants.json")) as f:
            mutants = f.read()
        real = {"src/broadcast_verify.rs": 75, "src/server/handlers.rs": 74, "src/tracker_host.rs": 72}
        provisional = nightly
        for file, floor in real.items():
            self.assertIn(f'"{file}": {floor}', nightly)
            provisional = provisional.replace(f'"{file}": {floor}', f'"{file}": 80')
        self.tree.write(".github/workflows/nightly.yml", provisional)
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml", nightly)
        self.tree.write("docs/scoreboard/mutants.json", mutants)
        code, out = self.checked(base)
        self.assertEqual(code, 0, out)
        for file, floor in real.items():
            self.assertIn(f"notice: floor:{file}: 80 at the base (provisional, unmeasured), {floor} in the tree", out)
        print(f"\n    exit 0: {tail(out)}", end=" ", file=sys.stderr)

    def test_a_removed_floor_is_refused(self):
        base = self.tree.values()
        self.tree.write(".github/workflows/nightly.yml",
                        NIGHTLY.replace('{"src/spend_gate.rs": 80,\n             ', "{"))
        self.refused_against(base, "a key may not vanish: floor:src/spend_gate.rs")
        self.refused_against(base, "floor:files: src/spend_gate.rs at the base, gone from the tree")

    def test_a_removed_wire_fixture_is_refused(self):
        base = self.tree.values()
        self.tree.write("tests/wire/MANIFEST.json", json.dumps({"fixtures": ["001.json"]}))
        self.refused_against(base, "fixtures:wire: 2 at the base, 1 in the tree")

    def test_a_removed_consumer_fixture_is_refused(self):
        base = self.tree.values()
        self.tree.remove("tests/wire_consumers/mcp/01.json")
        self.refused_against(base, "fixtures:consumers: 3 at the base, 2 in the tree")

    def test_a_removed_scenario_is_refused(self):
        base = self.tree.values()
        self.tree.remove("tests/e2e/scenarios/02-s.js")
        self.refused_against(base, "e2e:scenarios: 3 at the base, 2 in the tree")

    def test_a_removed_rule_is_refused(self):
        self.tree.write("tests/arch_other.rs", ARCH.replace("A4:", "A5:"))
        base = self.tree.values()
        self.tree.remove("tests/arch_other.rs")
        self.refused_against(base, "arch:rules: 2 at the base, 1 in the tree")

    def test_a_renamed_rule_is_refused(self):
        self.tree.write("tests/arch_other.rs", ARCH.replace("A4:", "A3:").replace('"src/a.rs"', '"src/d.rs"'))
        base = self.tree.values()
        self.tree.write("tests/arch_other.rs", ARCH.replace("A4:", "A9:").replace('"src/a.rs"', '"src/d.rs"'))
        self.refused_against(base, "a key may not vanish: allowlist:A3:files")
        self.refused_against(base, "a key may not vanish: allowlist:A3:sites")
        self.refused_against(base, "arch:rule-ids: A3 at the base, gone from the tree; a list may not lose an element")

    def test_a_rule_swapped_for_an_empty_one_is_refused(self):
        base = self.tree.values()
        self.tree.remove("tests/arch_no_unwrap.rs")
        self.tree.write("tests/arch_empty.rs", ARCH.replace("A4:", "A7:").split("const ALLOWED")[0]
                        + "const ALLOWED: &[(&str, usize, &str)] = &[\n];\n\n" + "const RULE" + ARCH.split("const RULE")[1].replace("A4:", "A7:"))
        self.assertIn("| arch rule A7 allowlist | 0 files, 0 sites |", self.table())
        self.refused_against(base, "a key may not vanish: allowlist:A4:sites")
        self.refused_against(base, "arch:rule-ids: A4 at the base, gone from the tree")

    def test_a_consumer_swapped_for_another_is_refused(self):
        base = self.tree.values()
        self.tree.remove("tests/wire_consumers/x402-skill")
        self.tree.write("tests/wire_consumers/other/00.json", "{}")
        self.refused_against(base, "consumers:names: x402-skill at the base, gone from the tree")

    def test_a_base_list_that_is_not_names_is_refused(self):
        path = os.path.join(self.tree.root, "base.json")
        with open(path, "w") as f:
            f.write(json.dumps({"schema": 1, "values": {"arch:rule-ids": 1}}))
        self.tree.run("--render")
        self.refused("values.arch:rule-ids: must be a list of names", "--check", "--base", path)

    def test_a_grown_allowlist_is_refused(self):
        base = self.tree.values()
        self.tree.write("tests/arch_no_unwrap.rs", ARCH.replace('"src/a.rs", 2,', '"src/a.rs", 4,'))
        self.refused_against(base, "allowlist:A4:sites: 5 at the base, 7 in the tree; an allowlist may not grow")

    def test_an_allowlist_with_a_new_file_is_refused(self):
        base = self.tree.values()
        self.tree.write("tests/arch_no_unwrap.rs",
                        ARCH.replace('    ("src/a.rs", 2,', '    ("src/z.rs", 1, "new"),\n    ("src/a.rs", 1,'))
        self.refused_against(base, "allowlist:A4:files: 2 at the base, 3 in the tree")

    def test_a_lost_hermetic_flag_is_refused(self):
        base = self.tree.values()
        self.tree.write("tests/e2e/lib/hermetic.js", "const enabled = false;\n")
        self.refused_against(base, "e2e:hermetic: 1 at the base, 0 in the tree")

    def test_a_malformed_base_is_refused(self):
        path = os.path.join(self.tree.root, "base.json")
        with open(path, "w") as f:
            f.write(json.dumps({"schema": 1, "values": {"floor:src/x.rs": "80"}}))
        self.tree.run("--render")
        self.refused("values.floor:src/x.rs: must be a number", "--check", "--base", path)
        with open(path, "w") as f:
            f.write("")
        self.refused(": (the file): not JSON", "--check", "--base", path)

    def test_a_base_without_check_is_a_usage_error(self):
        code, out = self.tree.run("--base", "x.json")
        self.assertEqual(code, 2, out)


class TheHermeticFlagIsTheKnobsRead(Case):
    """(G) The flag is a line of code reading process.env.E2E_HERMETIC, never a mention."""

    def flag(self):
        return [l for l in self.table().splitlines() if l.startswith("| hermetic flag")][0]

    def test_the_read_is_present(self):
        self.assertIn("| present |  | `tests/e2e/lib/hermetic.js` |", self.flag())

    def test_a_comment_is_not_the_knob(self):
        self.tree.write("tests/e2e/lib/hermetic.js",
                        "// const enabled = process.env.E2E_HERMETIC === '1';\n"
                        "/* process.env.E2E_HERMETIC\n   is read nowhere */\nconst enabled = false;\n")
        self.tree.write("tests/e2e/run.js", "// E2E_HERMETIC=1 node run.js\nconsole.log('E2E_HERMETIC');\n")
        self.assertIn("| absent |", self.flag())

    def test_the_bracket_read_is_the_knob(self):
        self.tree.remove("tests/e2e/lib/hermetic.js")
        self.tree.write("tests/e2e/run.js", "const on = process.env['E2E_HERMETIC'] === '1';\n")
        self.assertIn("| present |  | `tests/e2e/run.js` |", self.flag())


    def test_a_string_literal_is_not_the_knob(self):
        self.tree.write("tests/e2e/lib/hermetic.js",
                        "const a = 'process.env.E2E_HERMETIC';\n"
                        "const b = \"process.env['E2E_HERMETIC']\";\n"
                        "const c = `process.env.E2E_HERMETIC ${a}`;\n"
                        "const d = /process.env.E2E_HERMETIC/;\nconst enabled = false;\n")
        self.assertIn("| absent |", self.flag())
        print("\n    absent: the knob named in four string, template and regex literals", end=" ", file=sys.stderr)

    def test_a_url_in_a_string_does_not_hide_the_read(self):
        self.tree.write("tests/e2e/lib/hermetic.js",
                        "const u = 'http://x/*'; const on = process.env.E2E_HERMETIC === '1'; // */\n")
        self.assertIn("| present |", self.flag())


class AScenarioIsAModuleThatExportsRun(Case):
    """(I) As run.js calls scenario.run: a .js under scenarios/ without one is not counted."""

    def count(self):
        return [l for l in self.table().splitlines() if l.startswith("| e2e scenarios")][0]

    def test_files_without_a_run_are_not_counted(self):
        for name, text in (("90-empty.js", "module.exports = {};\n"),
                           ("91-comment.js", "// async run(a, b) {}\nmodule.exports = { name: 'x' };\n"),
                           ("92-string.js", "module.exports = { name: 'run', d: 'async run(a) {}' };\n"),
                           ("93-nested.js", "module.exports = { opts: { run() {} }, f: (run) => run };\n"),
                           ("94-local.js", "async function run() {}\nmodule.exports = { name: 'x' };\n")):
            self.tree.write(f"tests/e2e/scenarios/{name}", text)
        self.assertIn("| e2e scenarios | 3 |", self.count())
        print("\n    3: five .js files that export no run are not scenarios", end=" ", file=sys.stderr)

    def test_the_forms_of_run_are_counted(self):
        for name, text in (("90-prop.js", "module.exports.run = async (a, b) => ({});\n"),
                           ("91-key.js", "async function go() {}\nmodule.exports = { name: 'x', run: go };\n"),
                           ("92-short.js", "async function run() {}\nmodule.exports = { name: 'x', run };\n"),
                           ("93-quoted.js", "module.exports = { 'run': async () => {} };\n")):
            self.tree.write(f"tests/e2e/scenarios/{name}", text)
        self.assertIn("| e2e scenarios | 7 |", self.count())

    def test_a_scenario_emptied_of_its_run_is_refused(self):
        base = self.tree.values()
        self.tree.write("tests/e2e/scenarios/02-s.js", "// " + SCENARIO.replace("\n", "\n// "))
        self.tree.run("--render")
        code, out = self.tree.run("--check", "--base", base)
        self.assertEqual(code, 1, out)
        self.assertIn("e2e:scenarios: 3 at the base, 2 in the tree; it may not fall", out)
        print(f"\n    exit 1: {[l for l in out.splitlines() if 'e2e:scenarios' in l][0]}", end=" ", file=sys.stderr)


class TheConsumersCorpusIsReadWhereItLives(Case):
    """(H) tests/wire_consumers/<consumer>/<name>.json, and nothing else there."""

    def test_the_corpus_is_counted(self):
        self.assertIn("| consumers' fixtures | 3 over 2 consumers |  | `tests/wire_consumers/` |", self.table())

    def test_a_stray_file_is_refused(self):
        self.tree.write("tests/wire_consumers/README.md", "x")
        self.refused("tests/wire_consumers: README.md: every file is <consumer>/<name>.json")

    def test_a_nested_fixture_is_refused(self):
        self.tree.write("tests/wire_consumers/mcp/deep/01.json", "{}")
        self.refused("tests/wire_consumers: mcp/deep/01.json: every file is <consumer>/<name>.json")

    def test_the_old_home_is_not_read(self):
        self.tree.remove("tests/wire_consumers")
        self.tree.write("tests/wire/consumers/mcp/01.json", "{}")
        self.assertIn("| consumers' fixtures | not in the tree yet |", self.table())


if __name__ == "__main__":
    unittest.main(verbosity=2)
