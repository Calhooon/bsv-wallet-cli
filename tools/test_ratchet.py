#!/usr/bin/env python3
"""The planted cases of tools/ratchet's toolchain guard (#132): --record under
a toolchain other than the record's exits 1 and writes nothing, --record
--toolchain-move writes and names both toolchains, --record with no record
writes it, a success line names the toolchain, and --check still refuses a
toolchain other than the record's. Each case copies the tool into a git tree
of its own under $TMPDIR and runs it with a stub `cargo` and `rustc` on PATH
that print a chosen version and no warning, so no real build runs. Run by the
CI job ratchet: python3 tools/test_ratchet.py
"""

import os
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
# `cargo doc ...` print no message and succeed: every count is 0.
CARGO = """#!/bin/sh
for a in "$@"; do [ "$a" = --version ] && { echo "$STUB_CLIPPY"; exit 0; }; done
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
        env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"],
                   STUB_RUSTC=toolchain[0], STUB_CLIPPY=toolchain[1])
        env.pop("RUSTUP_TOOLCHAIN", None)
        r = subprocess.run([sys.executable, "-B", os.path.join(self.root, "tools", "ratchet"), *args],
                           cwd=self.root, env=env, capture_output=True, text=True)
        return r.returncode, (r.stdout + r.stderr).strip()

    def recorded(self, toolchain):
        """A record under `toolchain`, committed."""
        code, out = self.run(toolchain, "--record")
        assert code == 0, out
        self.commit("the record")
        return self.bytes()

    def bytes(self):
        with open(self.record, "rb") as f:
            return f.read()

    def cleanup(self):
        shutil.rmtree(self.root, ignore_errors=True)


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


if __name__ == "__main__":
    unittest.main(verbosity=2)
