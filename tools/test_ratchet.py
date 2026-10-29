#!/usr/bin/env python3
"""The planted cases of tools/ratchet's toolchain guard (#132): --record under
a toolchain other than the record's exits 1 and writes nothing, --record
--toolchain-move writes and names both toolchains, --record with no record
writes it, a success line names the toolchain, and --check still refuses a
toolchain other than the record's; a record that names no toolchain (none,
null, not an object) is named so under both, and --record refuses an older
schema before measuring (the PR 294 review, finding 1). The tool's
fingerprint (#127): --check refuses a record whose fingerprint is not the
tree's tool's; the same record (or one of an older schema) as --base warns and
the check passes on the record alone; a re-record after a tool edit carries
the new fingerprint; --record --schema-move rewrites an older schema's record.
Working-copy instrument edits are refused before measuring; malformed
fingerprints are named; a source edit keeps -dirty in the check's header.
The CI shell also checks a changed instrument's tree with the base's tool,
including a raise hidden by the PR's tool and the unchanged-instrument path.
With the base skipped, a record that is not the tree's measurement is
refused in either direction: raised by hand above the tree (the PR 296
re-check's plant, through the CI shell) or lowered by hand below it; a
correct re-record passes with the warning, and the same-instrument path keeps
its hint.
The base comparison (#126) covers a base above, equal to and below the tree,
a raised PR record, a key the base predates and an unrecorded key. Only a tree
below its own record gets the re-record hint. Non-object JSON records are
refused by name under --check, --record and --base.
The CI notice is limited to a path absent from a readable tree; invalid
refs, failed object reads and failed git show calls stop before measurement.
Each case copies the tool into a git tree
of its own under $TMPDIR and runs it with a stub `cargo` and `rustc` on PATH
that print a chosen version and no warning, so no real build runs. Run by the
CI job ratchet: python3 tools/test_ratchet.py
"""

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))

OLD = ("rustc 1.98.1 (48a229cea 2026-09-01)", "clippy 0.1.98 (48a229ceae 2026-09-01)")
NEW = ("rustc 1.95.0 (29483883e 2026-07-15)", "clippy 0.1.95 (29483883ee 2026-07-15)")

# `cargo clippy --version` prints the clippy line; `cargo clippy ...` and
# `cargo doc ...` print no message and succeed (every count is 0) and log
# their arguments to $STUB_LOG: a measurement leaves a line there.
CARGO = """#!/bin/sh
for a in "$@"; do [ "$a" = --version ] && { echo "$STUB_CLIPPY"; exit 0; }; done
echo "$*" >> "$STUB_LOG"
exit 0
"""
RUSTC = """#!/bin/sh
echo "$STUB_RUSTC"
"""


class Tree:
    """A committed tree holding the tool and one source file, under $TMPDIR."""

    def __init__(self):
        # Resolved, as the tool resolves its own ROOT (macOS's $TMPDIR is a symlink).
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="ratchet-case.", dir=os.environ.get("TMPDIR")))
        self.bin = os.path.join(self.root, ".stub-bin")
        os.makedirs(os.path.join(self.root, "tools"))
        os.makedirs(self.bin)
        shutil.copy(os.path.join(HERE, "ratchet"), os.path.join(self.root, "tools", "ratchet"))
        shutil.copytree(os.path.join(HERE, "ratchet.d"), os.path.join(self.root, "tools", "ratchet.d"))
        os.makedirs(os.path.join(self.root, "src"))
        os.makedirs(os.path.join(self.root, "docs"))
        with open(os.path.join(self.root, "docs", "README.md"), "w") as f:
            f.write("x\n")
        with open(os.path.join(self.root, "src", "lib.rs"), "w") as f:
            f.write("pub fn f() -> u8 {\n    Some(1).unwrap()\n}\n")
        for name, text in (("cargo", CARGO), ("rustc", RUSTC)):
            p = os.path.join(self.bin, name)
            with open(p, "w") as f:
                f.write(text)
            os.chmod(p, 0o755)
        with open(os.path.join(self.root, ".gitignore"), "w") as f:
            f.write(".stub-bin/\n")
        self.git("init", "-q")
        self.commit("the tree")

    @property
    def record(self):
        return os.path.join(self.root, "docs", "ratchets.json")

    def git(self, *args):
        subprocess.run(
            ["git", "-c", "user.name=case", "-c", "user.email=case@invalid",
             "-c", "commit.gpgsign=false", *args],
            cwd=self.root, check=True, capture_output=True,
        )

    def commit(self, msg):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)

    def run(self, toolchain, *args):
        r = subprocess.run([sys.executable, "-B", os.path.join(self.root, "tools", "ratchet"), *args],
                           cwd=self.root, env=self.env(toolchain), capture_output=True, text=True)
        return r.returncode, (r.stdout + r.stderr).strip()

    def env(self, toolchain):
        env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"],
                   STUB_RUSTC=toolchain[0], STUB_CLIPPY=toolchain[1],
                   STUB_LOG=os.path.join(self.bin, "measured"))
        env.pop("RUSTUP_TOOLCHAIN", None)
        return env

    def run_ci_step(self, rev):
        """The workflow's actual shell, with the same stub toolchain as run()."""
        with open(os.path.join(HERE, "..", ".github", "workflows", "ci.yml")) as f:
            workflow = f.read()
        step = workflow.split("      - name: No number above the base's record\n", 1)[1]
        lines = step.split("        run: |\n", 1)[1].splitlines()
        shell = []
        for line in lines:
            if line and not line.startswith("          "):
                break
            shell.append(line[10:])
        env = dict(self.env(OLD), REV=rev, RUNNER_TEMP=self.bin, GITHUB_WORKSPACE=self.root)
        r = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", "\n".join(shell)],
                           cwd=self.root, env=env, capture_output=True, text=True)
        return r.returncode, (r.stdout + r.stderr).strip()

    def fail_git(self, args, message, code):
        """Fail one Git operation, delegating every other call to real Git."""
        real_git = shutil.which("git")
        path = os.path.join(self.bin, "git")
        with open(path, "w") as f:
            f.write("#!/bin/sh\n"
                    f'if [ "$*" = {shlex.quote(" ".join(args))} ]; then\n'
                    f"  printf '%s\\n' {shlex.quote(message)} >&2\n"
                    f"  exit {code}\nfi\n"
                    f'exec {shlex.quote(real_git)} "$@"\n')
        os.chmod(path, 0o755)

    def recorded(self, toolchain):
        """A record under `toolchain`, committed."""
        code, out = self.run(toolchain, "--record")
        assert code == 0, out
        self.commit("the record")
        return self.bytes()

    def planted(self, **fields):
        """A record under OLD with `fields` set (a value of MISSING deletes
        the field), committed: an older record's shape."""
        self.recorded(OLD)
        with open(self.record) as f:
            rec = json.load(f)
        for k, v in fields.items():
            if v is MISSING:
                del rec[k]
            else:
                rec[k] = v
        with open(self.record, "w") as f:
            json.dump(rec, f, indent=2)
            f.write("\n")
        self.commit("the planted record")
        os.remove(os.path.join(self.bin, "measured"))  # the planting run's own
        return self.bytes()

    def measured(self):
        """Whether a run measured the tree (the stub cargo was asked to build)."""
        return os.path.exists(os.path.join(self.bin, "measured"))

    def bytes(self):
        with open(self.record, "rb") as f:
            return f.read()

    def json(self):
        with open(self.record) as f:
            return json.load(f)

    def sha(self, rel):
        with open(os.path.join(self.root, rel), "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()

    def edit_tool(self, rel="tools/ratchet"):
        """A change to the instrument (a comment appended), committed."""
        with open(os.path.join(self.root, rel), "a") as f:
            f.write("# an edit of the instrument\n")
        self.commit("the tool edited")

    def base(self, rec):
        """`rec` written as a base record outside the tree."""
        p = os.path.join(self.bin, "base-ratchets.json")
        with open(p, "w") as f:
            json.dump(rec, f, indent=2)
        return p

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


MISSING = object()


def tail(out):
    return out.splitlines()[-1] if out else ""


def named(tc):
    return f"{tc[0]}, {tc[1]}"


class Case(unittest.TestCase):
    def setUp(self):
        self.tree = Tree()
        self.addCleanup(self.tree.cleanup)

    def note(self, code, out):
        print(f"\n    exit {code}: {tail(out)}", end=" ", file=sys.stderr)


class RecordGuardsTheToolchain(Case):
    def test_record_under_another_toolchain_is_refused_and_writes_nothing(self):
        before = self.tree.recorded(OLD)
        code, out = self.tree.run(NEW, "--record")
        self.assertEqual(code, 1, out)
        self.assertIn(
            "ratchet: the toolchain differs from " + self.tree.record
            + "'s, the clippy numbers do not compare\n"
            f"  record:   {OLD[1]}\n  this run: {NEW[1]}",
            out,
        )
        self.assertIn("--record --toolchain-move", out)
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_a_rustc_alone_that_differs_is_refused(self):
        before = self.tree.recorded(OLD)
        code, out = self.tree.run((NEW[0], OLD[1]), "--record")
        self.assertEqual(code, 1, out)
        self.assertIn("the toolchain differs from", out)
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_toolchain_move_writes_and_names_both(self):
        before = self.tree.recorded(OLD)
        code, out = self.tree.run(NEW, "--record", "--toolchain-move")
        self.assertEqual(code, 0, out)
        self.assertIn(f"under {named(NEW)}", out)
        self.assertIn(f"ratchet: the toolchain moved from {named(OLD)} to {named(NEW)}", out)
        after = self.tree.bytes()
        self.assertNotEqual(after, before)
        self.assertIn(NEW[0].encode(), after)
        self.assertIn(NEW[1].encode(), after)
        self.note(code, out)

    def test_toolchain_move_with_no_move_is_refused(self):
        before = self.tree.recorded(OLD)
        code, out = self.tree.run(OLD, "--record", "--toolchain-move")
        self.assertEqual(code, 1, out)
        self.assertIn("--toolchain-move, but the toolchain is the record's", out)
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_toolchain_move_needs_record(self):
        code, out = self.tree.run(OLD, "--check", "--toolchain-move")
        self.assertEqual(code, 2, out)
        self.assertIn("--toolchain-move needs --record", out)
        self.note(code, out)

    def test_record_with_no_record_writes_it(self):
        self.assertFalse(os.path.exists(self.tree.record))
        code, out = self.tree.run(NEW, "--record")
        self.assertEqual(code, 0, out)
        self.assertIn(f"under {named(NEW)}", out)
        self.assertIn(NEW[1].encode(), self.tree.bytes())
        self.note(code, out)

    def test_record_under_the_same_toolchain_names_it(self):
        self.tree.recorded(OLD)
        code, out = self.tree.run(OLD, "--record")
        self.assertEqual(code, 0, out)
        self.assertRegex(out, r"ratchet: recorded \S+ at [0-9a-f]{12} under " + named(OLD).replace("(", r"\(").replace(")", r"\)"))
        self.assertNotIn("moved", out)
        self.note(code, out)


class CheckIsUnchanged(Case):
    def test_check_under_another_toolchain_is_refused(self):
        self.tree.recorded(OLD)
        code, out = self.tree.run(NEW, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(
            out,
            "ratchet: the toolchain differs from " + self.tree.record
            + "'s, the clippy numbers do not compare\n"
            f"  record:   {OLD[1]}\n  this run: {NEW[1]}",
        )
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 0, out)
        self.assertIn("ratchet: no number above the record", out)
        self.note(code, out)


class CheckAgainstBase(Case):
    def setUp(self):
        super().setUp()
        self.tree.recorded(OLD)

    def check_base(self, base):
        path = self.tree.base(base)
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertIn(f"ratchet: base record {path}", out)
        self.assertNotIn("::warning::", out)
        self.note(code, out)
        return code, out

    def test_a_base_above_the_tree_passes_without_a_rerecord_hint(self):
        base = self.tree.json()
        with open(os.path.join(self.tree.root, "src", "lib.rs"), "w") as f:
            f.write("pub fn f() -> u8 {\n    1\n}\n")
        self.tree.commit("an unwrap removed")
        self.tree.recorded(OLD)
        code, out = self.check_base(base)
        self.assertEqual(code, 0, out)
        self.assertNotIn("lower than the record (re-record in its own commit: tools/ratchet --record, "
                         "then python3 -B tools/scoreboard --render, "
                         "docs/NORTH-STAR.md's table committed with the record):", out)
        self.assertNotIn("re-record", out)
        self.assertTrue(out.endswith("ratchet: no number above the record or the base record"), out)

    def test_a_base_equal_to_the_tree_passes(self):
        code, out = self.check_base(self.tree.json())
        self.assertEqual(code, 0, out)
        self.assertNotIn("lower than the record (re-record in its own commit: tools/ratchet --record, "
                         "then python3 -B tools/scoreboard --render, "
                         "docs/NORTH-STAR.md's table committed with the record):", out)
        self.assertTrue(out.endswith("ratchet: no number above the record or the base record"), out)

    def test_a_base_below_the_tree_fails_with_raised(self):
        base = self.tree.json()
        base["ratchets"]["unwrap_expect_outside_tests"] = 0
        code, out = self.check_base(base)
        self.assertEqual(code, 1, out)
        self.assertIn("RAISED: the tree is above the base record", out)
        self.assertIn("unwrap_expect_outside_tests: 0 -> 1 (+1)", out)

    def test_a_pr_record_above_the_base_fails_even_when_the_tree_is_not(self):
        base = self.tree.json()
        rec = self.tree.json()
        rec["ratchets"]["unwrap_expect_outside_tests"] += 1
        with open(self.tree.record, "w") as f:
            json.dump(rec, f)
        self.tree.commit("the PR record raised above the base")
        code, out = self.check_base(base)
        self.assertEqual(code, 1, out)
        self.assertIn(f"RAISED: the record {self.tree.record} is above the base record", out)
        self.assertIn("unwrap_expect_outside_tests: 1 -> 2 (+1)", out)
        self.assertNotIn("RAISED: the tree", out)

    def test_a_key_the_base_predates_has_nothing_to_be_above(self):
        base = self.tree.json()
        del base["ratchets"]["unwrap_expect_outside_tests"]
        code, out = self.check_base(base)
        self.assertEqual(code, 0, out)
        self.assertIn("(record 1, base new)", out)
        self.assertNotIn("RAISED", out)
        self.assertNotIn("UNRECORDED", out)

    def test_a_key_missing_from_the_pr_record_is_unrecorded(self):
        base = self.tree.json()
        rec = self.tree.json()
        del rec["ratchets"]["unwrap_expect_outside_tests"]
        with open(self.tree.record, "w") as f:
            json.dump(rec, f)
        self.tree.commit("the PR record missing a ratchet")
        code, out = self.check_base(base)
        self.assertEqual(code, 1, out)
        self.assertIn(f"UNRECORDED: {self.tree.record} does not hold unwrap_expect_outside_tests"
                      " (record it in its own commit: tools/ratchet --record, "
                      "then python3 -B tools/scoreboard --render)", out)
        self.assertIn("1 -> absent (a ratchet dropped)", out)

    def test_the_rerecord_hint_uses_the_record_even_with_a_different_base(self):
        base = self.tree.json()
        base["ratchets"]["unwrap_expect_outside_tests"] = 3
        rec = self.tree.json()
        rec["ratchets"]["unwrap_expect_outside_tests"] = 2
        with open(self.tree.record, "w") as f:
            json.dump(rec, f)
        self.tree.commit("the record and base above the tree by different amounts")
        code, out = self.check_base(base)
        self.assertEqual(code, 0, out)
        self.assertIn("lower than the record (re-record in its own commit: tools/ratchet --record, "
                      "then python3 -B tools/scoreboard --render, "
                      "docs/NORTH-STAR.md's table committed with the record):", out)
        self.assertIn("unwrap_expect_outside_tests: 2 -> 1 (-1)", out)
        self.assertNotIn("unwrap_expect_outside_tests: 3 -> 1 (-2)", out)


class RecordMustBeAnObject(Case):
    def refuses(self, value, mode):
        self.tree.recorded(OLD)
        if mode == "--base":
            path = self.tree.base(value)
            args = ("--check", "--base", path)
        else:
            path = self.tree.record
            with open(path, "w") as f:
                json.dump(value, f)
            self.tree.commit("a non-object record")
            args = (mode,)
        os.remove(os.path.join(self.tree.bin, "measured"))
        with open(path, "rb") as f:
            before = f.read()
        code, out = self.tree.run(OLD, *args)
        self.assertEqual(code, 1, out)
        self.assertEqual(out, f"ratchet: {path} is not a record"
                         " (a JSON object with schema, commit, toolchain, ratchets)")
        self.assertNotIn("Traceback", out)
        with open(path, "rb") as f:
            self.assertEqual(f.read(), before)
        if mode == "--record":
            self.assertFalse(self.tree.measured())
        self.note(code, out)

    def test_check_refuses_an_array_record_by_name(self):
        self.refuses([], "--check")

    def test_check_refuses_a_string_record_by_name(self):
        self.refuses("record", "--check")

    def test_check_refuses_a_null_record_by_name(self):
        self.refuses(None, "--check")

    def test_record_refuses_an_array_record_by_name(self):
        self.refuses([], "--record")

    def test_record_refuses_a_string_record_by_name(self):
        self.refuses("record", "--record")

    def test_record_refuses_a_null_record_by_name(self):
        self.refuses(None, "--record")

    def test_base_refuses_an_array_record_by_name(self):
        self.refuses([], "--base")

    def test_base_refuses_a_string_record_by_name(self):
        self.refuses("record", "--base")

    def test_base_refuses_a_null_record_by_name(self):
        self.refuses(None, "--base")



# The review of PR 294, finding 1: a record that names no toolchain (no field,
# null, or not an object holding rustc and clippy) is named so, never `None`
# and never a traceback; and --record refuses an older schema before measuring.
SCHEMA = 2
NO_TOOLCHAIN = f"names no toolchain (schema {SCHEMA})"


class OlderRecordIsNamed(Case):
    def refused_by_check(self, **fields):
        self.tree.planted(**fields)
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertNotIn("Traceback", out)
        self.assertEqual(
            out,
            "ratchet: the toolchain differs from " + self.tree.record
            + "'s, the clippy numbers do not compare\n"
            f"  record:   {NO_TOOLCHAIN}\n  this run: {OLD[1]}",
        )
        self.note(code, out)

    def refused_by_record(self, **fields):
        before = self.tree.planted(**fields)
        code, out = self.tree.run(OLD, "--record")
        self.assertEqual(code, 1, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(f"  record:   {NO_TOOLCHAIN}\n  this run: {OLD[1]}", out)
        self.assertIn("--record --toolchain-move", out)
        self.assertEqual(self.tree.bytes(), before)
        self.assertFalse(self.tree.measured())
        self.note(code, out)

    def moved_by_record(self, **fields):
        before = self.tree.planted(**fields)
        code, out = self.tree.run(OLD, "--record", "--toolchain-move")
        self.assertEqual(code, 0, out)
        self.assertNotIn("Traceback", out)
        self.assertIn(f"ratchet: the toolchain moved from no toolchain (schema {SCHEMA}) to {named(OLD)}", out)
        self.assertNotIn("None", out)
        self.assertNotEqual(self.tree.bytes(), before)
        self.assertIn(OLD[1].encode(), self.tree.bytes())
        self.note(code, out)

    def test_check_names_a_missing_toolchain(self):
        self.refused_by_check(toolchain=MISSING)

    def test_check_names_a_null_toolchain(self):
        self.refused_by_check(toolchain=None)

    def test_check_names_a_non_object_toolchain(self):
        self.refused_by_check(toolchain="rustc 1.98.1")

    def test_check_names_an_object_without_clippy(self):
        self.refused_by_check(toolchain={"rustc": OLD[0]})

    def test_record_refuses_a_missing_toolchain(self):
        self.refused_by_record(toolchain=MISSING)

    def test_record_refuses_a_null_toolchain(self):
        self.refused_by_record(toolchain=None)

    def test_record_refuses_a_non_object_toolchain(self):
        self.refused_by_record(toolchain="rustc 1.98.1")

    def test_toolchain_move_names_a_missing_toolchain(self):
        self.moved_by_record(toolchain=MISSING)

    def test_toolchain_move_names_a_null_toolchain(self):
        self.moved_by_record(toolchain=None)

    def test_toolchain_move_names_a_non_object_toolchain(self):
        self.moved_by_record(toolchain="rustc 1.98.1")

    def refused_schema(self, *args):
        before = self.tree.planted(schema=0, toolchain=MISSING)
        code, out = self.tree.run(OLD, *args)
        self.assertEqual(code, 1, out)
        refusal = f"ratchet: {self.tree.record} is schema 0, the tool is {SCHEMA}"
        if "--record" in args:
            self.assertEqual(out, refusal + "\nratchet: --record refuses it;"
                             " a schema move is its own commit: --record --schema-move")
            self.assertFalse(self.tree.measured())
        else:
            self.assertEqual(out, refusal)
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_record_refuses_an_older_schema_before_measuring(self):
        self.refused_schema("--record")

    def test_toolchain_move_refuses_an_older_schema_before_measuring(self):
        self.refused_schema("--record", "--toolchain-move")

    def test_check_refuses_an_older_schema_with_the_same_message(self):
        self.refused_schema("--check")



# #127: the record names its instrument, the sha256 of tools/ratchet and of
# tools/ratchet.d/clippy.toml as HEAD holds them.
TOOL_DIFFERS = (
    "ratchet: the tool differs from {}'s record (tools/ratchet or"
    " tools/ratchet.d/clippy.toml changed since it was recorded): re-record"
    " in its own commit after the tool's, in the same PR, with the reason in its message"
)
NO_INSTRUMENT = (
    "ratchet: the tool differs from {}'s record (names no instrument (schema 2)): re-record"
    " in its own commit after the tool's, in the same PR, with the reason in its message"
)
BASE_SKIPPED = (
    "::warning::ratchet: the base record {} was made by another tool{}:"
    " the base comparison is skipped; the PR's re-record commit states the reason"
)
OTHER_TOOL = {"ratchet": "0" * 64, "clippy_toml": "0" * 64}


class TheToolIsFingerprinted(Case):
    def test_record_writes_the_fingerprint_of_heads_tool(self):
        self.tree.recorded(OLD)
        rec = self.tree.json()
        self.assertEqual(rec["schema"], SCHEMA)
        self.assertEqual(rec["tool"], {"ratchet": self.tree.sha("tools/ratchet"),
                                       "clippy_toml": self.tree.sha("tools/ratchet.d/clippy.toml")})
        self.assertEqual(list(rec)[:5], ["schema", "commit", "toolchain", "tool", "ratchets"])
        self.note(0, str(rec["tool"]))

    def test_check_refuses_a_record_of_another_tool(self):
        before = self.tree.planted(tool=OTHER_TOOL)
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, TOOL_DIFFERS.format(self.tree.record))
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_check_refuses_a_record_with_no_fingerprint(self):
        self.refused_instrument(MISSING)

    def refused_instrument(self, tool):
        before = self.tree.planted(tool=tool)
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, NO_INSTRUMENT.format(self.tree.record))
        self.assertEqual(self.tree.bytes(), before)
        self.assertNotIn("Traceback", out)
        self.note(code, out)

    def test_check_names_a_null_instrument(self):
        self.refused_instrument(None)

    def test_check_names_a_non_object_instrument(self):
        self.refused_instrument("sha256")

    def test_check_names_an_instrument_without_clippy_toml(self):
        self.refused_instrument({"ratchet": "0" * 64})

    def test_check_names_an_instrument_with_a_non_string_hash(self):
        self.refused_instrument(dict(OTHER_TOOL, ratchet=7))

    def test_check_names_an_instrument_with_a_malformed_hash(self):
        self.refused_instrument(dict(OTHER_TOOL, clippy_toml="not a sha256"))

    def test_a_tool_edit_without_a_re_record_fails_check(self):
        # The issue's exit test: a PR that edits the tool without re-recording.
        self.tree.recorded(OLD)
        self.tree.edit_tool()
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, TOOL_DIFFERS.format(self.tree.record))
        self.note(code, out)

    def test_a_clippy_toml_edit_without_a_re_record_fails_check(self):
        self.tree.recorded(OLD)
        self.tree.edit_tool("tools/ratchet.d/clippy.toml")
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, TOOL_DIFFERS.format(self.tree.record))
        self.note(code, out)

    def test_a_re_record_after_a_tool_edit_carries_the_new_fingerprint(self):
        self.tree.recorded(OLD)
        old_tool = self.tree.json()["tool"]
        self.tree.edit_tool()
        code, out = self.tree.run(OLD, "--record")
        self.assertEqual(code, 0, out)
        self.tree.commit("the re-record")
        tool = self.tree.json()["tool"]
        self.assertNotEqual(tool["ratchet"], old_tool["ratchet"])
        self.assertEqual(tool["ratchet"], self.tree.sha("tools/ratchet"))
        self.assertEqual(tool["clippy_toml"], old_tool["clippy_toml"])
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 0, out)
        self.assertIn("ratchet: no number above the record", out)
        self.note(code, out)

    def test_fingerprint_prints_heads_hashes_without_measuring(self):
        code, out = self.tree.run(OLD, "--fingerprint")
        self.assertEqual(code, 0, out)
        self.assertEqual(json.loads(out), {
            "ratchet": self.tree.sha("tools/ratchet"),
            "clippy_toml": self.tree.sha("tools/ratchet.d/clippy.toml"),
        })
        self.assertFalse(self.tree.measured())
        self.note(code, out)

    def refused_working_copy(self, rel):
        self.tree.recorded(OLD)
        os.remove(os.path.join(self.tree.bin, "measured"))
        with open(os.path.join(self.tree.root, rel), "a") as f:
            f.write("# uncommitted\n")
        for args in ((), ("--check",), ("--fingerprint",)):
            with self.subTest(args=args):
                code, out = self.tree.run(OLD, *args)
                self.assertEqual(code, 1, out)
                self.assertEqual(out, f"ratchet: {rel} differs from HEAD:"
                                 " commit or remove the instrument edit before measuring")
                self.assertFalse(self.tree.measured())
        self.note(code, out)

    def test_fingerprint_refuses_an_uncommitted_tool_edit_before_measuring(self):
        self.refused_working_copy("tools/ratchet")

    def test_fingerprint_refuses_an_uncommitted_clippy_toml_edit_before_measuring(self):
        self.refused_working_copy("tools/ratchet.d/clippy.toml")

    def test_check_keeps_the_dirty_suffix_for_an_uncommitted_source_edit(self):
        self.tree.recorded(OLD)
        with open(os.path.join(self.tree.root, "src", "lib.rs"), "a") as f:
            f.write("// uncommitted source\n")
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 0, out)
        self.assertRegex(out.splitlines()[0], r"tree at [0-9a-f]{12}-dirty$")
        self.note(code, out.splitlines()[0])

    def base_is_skipped(self, base, why):
        self.tree.recorded(OLD)
        path = self.tree.base(base)
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 0, out)
        self.assertIn(BASE_SKIPPED.format(path, why), out)
        self.assertNotIn("ratchet: base record", out)
        self.assertNotIn(", base ", out)
        self.assertTrue(out.endswith("ratchet: no number above the record"), out)
        self.note(code, out)

    def test_a_base_of_another_tool_warns_and_checks_the_record_alone(self):
        self.tree.recorded(OLD)
        base = dict(self.tree.json(), tool=OTHER_TOOL)
        base["ratchets"] = {k: 0 for k in base["ratchets"]}  # would fail if compared
        self.base_is_skipped(base, "")

    def test_a_base_of_an_older_schema_warns_and_checks_the_record_alone(self):
        self.tree.recorded(OLD)
        base = dict(self.tree.json(), schema=1)
        del base["tool"]
        base["ratchets"] = {k: 0 for k in base["ratchets"]}
        self.base_is_skipped(base, f" (schema 1, the tool is {SCHEMA})")

    def test_a_skipped_base_still_fails_a_tree_above_its_record(self):
        self.tree.planted(ratchets={"unwrap_expect_outside_tests": 0})
        path = self.tree.base(dict(self.tree.json(), tool=OTHER_TOOL))
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 1, out)
        self.assertIn("::warning::ratchet: the base record", out)
        self.assertIn("RAISED: the tree is above the record", out)
        self.note(code, tail(out))

    def test_a_base_of_the_same_tool_is_compared(self):
        self.tree.recorded(OLD)
        base = self.tree.json()
        base["ratchets"] = dict(base["ratchets"], unwrap_expect_outside_tests=0)
        path = self.tree.base(base)
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 1, out)
        self.assertNotIn("::warning::", out)
        self.assertIn(f"RAISED: the tree is above the base record {path}", out)
        self.note(code, tail(out))

    def test_a_base_of_a_newer_schema_is_refused(self):
        self.tree.recorded(OLD)
        path = self.tree.base(dict(self.tree.json(), schema=SCHEMA + 1))
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 1, out)
        self.assertEqual(out, f"ratchet: {path} is schema {SCHEMA + 1}, the tool is {SCHEMA}")
        self.note(code, out)

    def test_a_base_of_another_toolchain_is_still_refused(self):
        self.tree.recorded(OLD)
        base = dict(self.tree.json(), toolchain={"rustc": NEW[0], "clippy": NEW[1]})
        path = self.tree.base(base)
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 1, out)
        self.assertIn(f"the toolchain differs from {path}'s", out)
        self.note(code, out.splitlines()[0])

    def test_check_refuses_an_older_schema_on_the_baseline_by_name(self):
        before = self.tree.planted(schema=1, tool=MISSING)
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, f"ratchet: {self.tree.record} is schema 1, the tool is {SCHEMA}")
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)


class SchemaMove(Case):
    def test_schema_move_rewrites_an_older_schema_with_the_fingerprint(self):
        self.tree.planted(schema=1, tool=MISSING)
        code, out = self.tree.run(OLD, "--record", "--schema-move")
        self.assertEqual(code, 0, out)
        self.assertIn(f"ratchet: the schema moved from 1 to {SCHEMA}", out)
        rec = self.tree.json()
        self.assertEqual(rec["schema"], SCHEMA)
        self.assertEqual(rec["tool"]["ratchet"], self.tree.sha("tools/ratchet"))
        self.tree.commit("the re-record")
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 0, out)
        self.note(code, tail(out))

    def test_schema_move_still_guards_the_toolchain(self):
        before = self.tree.planted(schema=1, tool=MISSING)
        code, out = self.tree.run(NEW, "--record", "--schema-move")
        self.assertEqual(code, 1, out)
        self.assertIn("the toolchain differs from", out)
        self.assertEqual(self.tree.bytes(), before)
        self.assertFalse(self.tree.measured())
        self.note(code, out.splitlines()[0])

    def test_schema_move_with_no_move_is_refused(self):
        before = self.tree.recorded(OLD)
        os.remove(os.path.join(self.tree.bin, "measured"))
        code, out = self.tree.run(OLD, "--record", "--schema-move")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, f"ratchet: --schema-move, but {self.tree.record} is the tool's schema {SCHEMA}")
        self.assertEqual(self.tree.bytes(), before)
        self.assertFalse(self.tree.measured())
        self.note(code, out)

    def test_schema_move_refuses_a_newer_schema(self):
        before = self.tree.planted(schema=SCHEMA + 1)
        code, out = self.tree.run(OLD, "--record", "--schema-move")
        self.assertEqual(code, 1, out)
        self.assertEqual(out, f"ratchet: {self.tree.record} is schema {SCHEMA + 1}, the tool is {SCHEMA}")
        self.assertEqual(self.tree.bytes(), before)
        self.note(code, out)

    def test_schema_move_needs_record(self):
        code, out = self.tree.run(OLD, "--check", "--schema-move")
        self.assertEqual(code, 2, out)
        self.assertIn("--schema-move needs --record", out)
        self.note(code, out)


class BaseRecordLookup(Case):
    def setUp(self):
        super().setUp()
        self.tree.recorded(OLD)
        self.tree.git("tag", "case-base")
        os.remove(os.path.join(self.tree.bin, "measured"))

    def missing_record(self):
        code, out = self.tree.run_ci_step("HEAD~1")
        self.assertEqual(code, 0, out)
        self.assertIn("::notice::no base record (HEAD~1)", out)
        self.assertIn("ratchet: no number above the record", out)
        self.assertNotIn("ratchet: the base record is", out)
        self.assertNotIn("::error::", out)
        self.assertTrue(self.tree.measured())
        self.note(code, out)

    def failed_lookup(self, rev, operation):
        code, out = self.tree.run_ci_step(rev)
        self.assertEqual(code, 1, out)
        self.assertIn(f"::error::ratchet: git {operation} failed for {rev}:docs/ratchets.json", out)
        self.assertNotIn("::notice::", out)
        self.assertFalse(self.tree.measured())
        self.note(code, out)

    def test_ci_an_absent_record_takes_the_notice_and_checks_the_record_alone(self):
        self.missing_record()

    def test_ci_an_absent_record_with_cat_file_exit_one_takes_the_notice(self):
        for message in ("fatal: path 'docs/ratchets.json' does not exist in 'HEAD~1'",
                        "fatal: Not a valid object name HEAD~1:docs/ratchets.json",
                        "fatal: path 'docs/ratchets.json' exists on disk, but not in 'HEAD~1'"):
            with self.subTest(message=message):
                self.tree.fail_git(["cat-file", "-e", "HEAD~1:docs/ratchets.json"], message, 1)
                self.missing_record()

    def test_ci_a_bogus_ref_fails_by_name_without_a_notice(self):
        self.failed_lookup("not-a-real-ratchet-ref", "cat-file -e")

    def test_ci_a_cat_file_fault_fails_by_name_without_a_notice(self):
        self.tree.fail_git(["cat-file", "-e", "case-base:docs/ratchets.json"],
                           "fatal: unable to read object: input/output error", 1)
        self.failed_lookup("case-base", "cat-file -e")

    def test_ci_an_unreadable_existing_object_is_not_an_absent_path(self):
        self.tree.fail_git(["cat-file", "-e", "case-base:docs/ratchets.json"],
                           "fatal: Not a valid object name case-base:docs/ratchets.json", 1)
        self.failed_lookup("case-base", "cat-file -e")

    def test_ci_a_git_show_transport_failure_fails_by_name_without_a_notice(self):
        self.tree.fail_git(["show", "case-base:docs/ratchets.json"],
                           "fatal: simulated transport failure", 1)
        self.failed_lookup("case-base", "show")

    def test_ci_a_git_show_corrupt_object_fails_by_name_without_a_notice(self):
        self.tree.fail_git(["show", "case-base:docs/ratchets.json"],
                           "fatal: simulated corrupt object", 128)
        self.failed_lookup("case-base", "show")


class BaseInstrumentGuards(Case):
    def test_ci_measures_with_the_base_instrument_when_fingerprints_differ(self):
        self.tree.recorded(OLD)
        self.tree.git("tag", "case-base")
        self.tree.edit_tool("tools/ratchet.d/clippy.toml")
        self.tree.recorded(OLD)
        code, out = self.tree.run_ci_step("case-base")
        self.assertEqual(code, 0, out)
        self.assertIn("checking the PR tree with case-base's instrument", out)
        self.assertIn("::warning::ratchet: the base record", out)
        self.assertEqual(out.count("ratchet: no number above the record"), 2, out)
        self.assertFalse(any(name.startswith("ratchet-base.") for name in os.listdir(self.tree.bin)))
        self.note(code, "both the base's instrument and the PR's instrument pass")

    def test_ci_base_instrument_rejects_a_raise_hidden_by_the_pr_instrument(self):
        self.tree.recorded(OLD)
        self.tree.git("tag", "case-base")
        tool = os.path.join(self.tree.root, "tools", "ratchet")
        with open(tool) as f:
            text = f.read()
        with open(tool, "w") as f:
            f.write(text.replace('\nif __name__ == "__main__":',
                                 '\nunwrap_expect = lambda *args: 0\n\nif __name__ == "__main__":'))
        with open(os.path.join(self.tree.root, "src", "lib.rs"), "w") as f:
            f.write("pub fn f() -> u8 { Some(1).unwrap() + Some(2).unwrap() }\n")
        self.tree.commit("the PR instrument hides an extra unwrap")
        self.tree.recorded(OLD)
        self.assertEqual(self.tree.json()["ratchets"]["unwrap_expect_outside_tests"], 0)
        code, out = self.tree.run(OLD, "--check")
        self.assertEqual(code, 0, out)
        code, out = self.tree.run_ci_step("case-base")
        self.assertEqual(code, 1, out)
        self.assertIn("RAISED: the tree is above the record", out)
        self.assertIn("unwrap_expect_outside_tests: 1 -> 2 (+1)", out)
        self.assertFalse(any(name.startswith("ratchet-base.") for name in os.listdir(self.tree.bin)))
        self.note(code, "RAISED: the base's instrument counts the unwrap the PR hid")

    def test_ci_uses_the_ordinary_path_when_fingerprints_match(self):
        self.tree.recorded(OLD)
        self.tree.git("tag", "case-base")
        code, out = self.tree.run_ci_step("case-base")
        self.assertEqual(code, 0, out)
        self.assertNotIn("checking the PR tree with", out)
        self.assertNotIn("::warning::", out)
        self.assertIn("ratchet: no number above the record or the base record", out)
        self.note(code, tail(out))


    def rerecorded_after_a_tool_edit(self, **ratchets):
        """A base record, a clippy.toml edit and a re-record, committed, then
        `ratchets` set by hand in the record and committed; the base's path."""
        self.tree.recorded(OLD)
        self.tree.git("tag", "case-base")
        path = self.tree.base(self.tree.json())
        self.tree.edit_tool("tools/ratchet.d/clippy.toml")
        self.tree.recorded(OLD)
        if ratchets:
            rec = self.tree.json()
            rec["ratchets"].update(ratchets)
            with open(self.tree.record, "w") as f:
                json.dump(rec, f, indent=2)
                f.write("\n")
            self.tree.commit("the record edited by hand")
        return path

    def test_a_record_raised_by_hand_across_instruments_is_refused_in_ci(self):
        # The PR 296 re-check's plant: the base's instrument passes the tree,
        # and the PR's tool, skipping the base, must refuse the raised record.
        self.rerecorded_after_a_tool_edit(clippy_pedantic=900)
        code, out = self.tree.run_ci_step("case-base")
        self.assertEqual(code, 1, out)
        self.assertIn("checking the PR tree with case-base's instrument", out)
        self.assertIn("::warning::ratchet: the base record", out)
        self.assertIn(f"RAISED: the base record {self.tree.bin}/base-ratchets.json is another instrument's", out)
        self.assertIn("  clippy_pedantic: record 900, tree 0", out)
        self.note(code, "clippy_pedantic: record 900, tree 0")

    def test_a_record_lowered_by_hand_across_instruments_is_refused(self):
        path = self.rerecorded_after_a_tool_edit(unwrap_expect_outside_tests=0)
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 1, out)
        self.assertIn(f"RAISED: the base record {path} is another instrument's", out)
        self.assertIn("  unwrap_expect_outside_tests: record 0, tree 1", out)
        self.note(code, "unwrap_expect_outside_tests: record 0, tree 1")

    def test_a_re_record_across_instruments_passes_with_the_warning(self):
        path = self.rerecorded_after_a_tool_edit()
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 0, out)
        self.assertIn(BASE_SKIPPED.format(path, ""), out)
        self.assertNotIn("RAISED", out)
        self.assertTrue(out.endswith("ratchet: no number above the record"), out)
        self.note(code, tail(out))

    def test_the_same_instrument_keeps_its_hint_for_a_tree_below_the_record(self):
        self.tree.recorded(OLD)
        path = self.tree.base(self.tree.json())
        with open(os.path.join(self.tree.root, "src", "lib.rs"), "w") as f:
            f.write("pub fn f() -> u8 {\n    1\n}\n")
        self.tree.commit("an unwrap removed, not yet re-recorded")
        code, out = self.tree.run(OLD, "--check", "--base", path)
        self.assertEqual(code, 0, out)
        self.assertNotIn("::warning::", out)
        self.assertNotIn("another instrument's", out)
        self.assertIn("lower than the record (re-record in its own commit: tools/ratchet --record, "
                      "then python3 -B tools/scoreboard --render, "
                      "docs/NORTH-STAR.md's table committed with the record):", out)
        self.assertIn("unwrap_expect_outside_tests: 1 -> 0 (-1)", out)
        self.assertTrue(out.endswith("ratchet: no number above the record or the base record"), out)
        self.note(code, tail(out))


if __name__ == "__main__":
    unittest.main(verbosity=2)
