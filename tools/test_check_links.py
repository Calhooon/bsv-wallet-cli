#!/usr/bin/env python3
"""The planted cases of tools/check-links (#77): a broken link under docs/'s
subdirectories, an angle-bracket target, and an anchor that names no heading,
each found, and the links it must let through. Each case runs a copy of the
tool in a git tree of its own under $TMPDIR (git add, no commit, no network).
Run by the CI job ratchet: python3 tools/test_check_links.py
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "check-links")

TARGET = """# The target

## 1. Why

## The `spend` gate (hot)

## Twice

## Twice

Setext heading
--------------

<a id="kept"></a>

```
## not a heading
```
"""


class Case(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="check-links-case.", dir=os.environ.get("TMPDIR"))
        self.addCleanup(shutil.rmtree, self.root, True)
        os.makedirs(os.path.join(self.root, "tools"))
        shutil.copy(TOOL, os.path.join(self.root, "tools", "check-links"))
        subprocess.run(["git", "init", "-q", self.root], check=True)
        self.write("docs/target.md", TARGET)
        self.write("docs/with space.md", "# Spaced\n")
        self.write("src/lib.rs", "")

    def write(self, name, text):
        p = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
        subprocess.run(["git", "-C", self.root, "add", name], check=True)

    def run_tool(self):
        r = subprocess.run([sys.executable, "-B", os.path.join(self.root, "tools", "check-links")],
                           capture_output=True, text=True)
        return r.returncode, (r.stdout + r.stderr).strip()

    def passes(self, text, name="docs/a.md"):
        self.write(name, text)
        code, out = self.run_tool()
        self.assertEqual(code, 0, out)

    def broken(self, text, needle, name="docs/a.md"):
        self.write(name, text)
        code, out = self.run_tool()
        self.assertEqual(code, 1, out)
        self.assertIn(needle, out)
        print(f"\n    exit 1: {out.splitlines()[0]}", end=" ", file=sys.stderr)


class TheSubdirectoriesAreChecked(Case):
    def test_a_lane_brief_is_checked(self):
        self.broken("[gone](../../nowhere.md)\n", "docs/lanes/brief.md:1: ../../nowhere.md does not resolve",
                    name="docs/lanes/brief.md")

    def test_a_lane_brief_link_that_resolves_passes(self):
        self.passes("[t](../target.md) [src](../../src/lib.rs)\n", name="docs/lanes/brief.md")


class AngleTargetsAreRead(Case):
    def test_an_angle_target_that_resolves_passes(self):
        self.passes("[s](<with space.md>) and [t](<target.md#1-why>)\n")

    def test_an_angle_target_that_does_not_resolve_is_broken(self):
        self.broken("[s](<no such file.md>)\n", "docs/a.md:1: no such file.md does not resolve")

    def test_an_angle_reference_definition_is_read(self):
        self.broken("[s]: <no such file.md>\n", "docs/a.md:1: no such file.md does not resolve")


class AnchorsNameHeadings(Case):
    def test_headings_by_githubs_slug_pass(self):
        self.passes("[a](target.md#the-target) [b](target.md#1-why) [c](target.md#the-spend-gate-hot)\n"
                    "[d](target.md#twice) [e](target.md#twice-1) [f](target.md#setext-heading)\n"
                    "[g](target.md#kept) [h](#mine) [src](../src/lib.rs#L1)\n\n## Mine\n")

    def test_an_anchor_with_no_heading_is_broken(self):
        self.broken("[a](target.md#no-such-heading)\n",
                    "docs/a.md:1: target.md#no-such-heading: docs/target.md has no heading or id with the anchor")

    def test_a_third_twice_is_broken(self):
        self.broken("[a](target.md#twice-2)\n", "target.md#twice-2: docs/target.md has no heading")

    def test_a_heading_in_a_fence_is_not_an_anchor(self):
        self.broken("[a](target.md#not-a-heading)\n", "target.md#not-a-heading: docs/target.md has no heading")

    def test_an_own_anchor_with_no_heading_is_broken(self):
        self.broken("[a](#nowhere)\n\n## Somewhere\n", "docs/a.md:1: #nowhere: docs/a.md has no heading")


if __name__ == "__main__":
    unittest.main(verbosity=2)
