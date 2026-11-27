#!/usr/bin/env python3
"""The proof of the direction rule's two checks (.github/ci/lines.sh; RELEASING.md, "The
direction rule").

Each case builds a scratch repository under a temporary directory (TMPDIR is honoured), with a
`main` and a `beta` grown from one commit, plants the commits its row names and runs
`bash .github/ci/lines.sh` with MAIN_REF, BETA_REF, HEAD_REF, MAIN_BASELINE, PR_BASE and
PR_HEAD set. It asserts the exit status and, where the row names one, a line of the output.
Standard library and git only. It prints `lines: N cases` and exits non-zero on any miss.

    python3 -B .github/ci/test_lines.py
"""

import os
import shutil
import subprocess
import sys
import tempfile

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "lines.sh")

CARGO_TOML = """[package]
name = "app"
version = "1.0.0"

[dependencies]
bsv-rs = "=0.8.1"
serde = "1.0.1"
"""

CARGO_LOCK = """[[package]]
name = "app"
version = "1.0.0"

[[package]]
name = "bsv-rs"
version = "0.8.1"

[[package]]
name = "serde"
version = "1.0.1"
"""

CHANGELOG = """# Changelog

## [Unreleased]

## [1.0.0] - 2026-01-01

- The beginning.
"""

SECTION = "## [1.0.1] - 2026-02-02\n\n- A fix of the door.\n\n"
UNKNOWN = "deadbeef" * 5


class Repo:
    """One scratch repository: `main` and `beta` from a shared commit."""

    def __init__(self, root, name):
        self.dir = os.path.join(root, name)
        os.makedirs(self.dir)
        self.clock = 1_700_000_000
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        self.env.update(
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_SYSTEM=os.devnull,
            GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_NAME="lines",
            GIT_AUTHOR_EMAIL="lines@example.invalid",
            GIT_COMMITTER_NAME="lines",
            GIT_COMMITTER_EMAIL="lines@example.invalid",
        )
        self.git("init", "-q", "-b", "main")
        self.write("Cargo.toml", CARGO_TOML)
        self.write("Cargo.lock", CARGO_LOCK)
        self.write("CHANGELOG.md", CHANGELOG)
        self.write("changelog.d/README.md", "Fragments.\n")
        self.write("README.md", "The front page.\n")
        self.write("HANDOFF.md", "The record.\n")
        self.write("src.txt", "one\n")
        self.base = self.commit("the shared commit")
        self.git("branch", "beta")
        # the lines part: one commit of beta's own, so beta is never main's ancestor
        self.on("beta")
        self.write("beta.txt", "beta's own\n")
        self.commit("beta: its own work")
        self.on("main")

    def git(self, *args, check=True):
        self.clock += 60
        env = dict(self.env)
        env["GIT_AUTHOR_DATE"] = env["GIT_COMMITTER_DATE"] = "%d +0000" % self.clock
        done = subprocess.run(
            ["git", *args], cwd=self.dir, env=env, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        if check and done.returncode != 0:
            raise RuntimeError("git %s: %s" % (" ".join(args), done.stdout))
        return done.stdout.strip()

    def on(self, branch, start=None):
        if start is None:
            self.git("switch", "-q", branch)
        else:
            self.git("switch", "-q", "-c", branch, start)

    def path(self, rel):
        return os.path.join(self.dir, rel)

    def write(self, rel, text):
        os.makedirs(os.path.dirname(self.path(rel)) or self.dir, exist_ok=True)
        with open(self.path(rel), "w", encoding="utf-8") as out:
            out.write(text)

    def read(self, rel):
        with open(self.path(rel), encoding="utf-8") as src:
            return src.read()

    def edit(self, rel, old, new):
        text = self.read(rel)
        if old not in text:
            raise RuntimeError("%s does not hold %r" % (rel, old))
        self.write(rel, text.replace(old, new, 1))

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def sha(self, ref):
        return self.git("rev-parse", ref)

    def lines(self, check, **env):
        """Run the script; return (exit status, output)."""
        run_env = {k: v for k, v in self.env.items()
                   if k not in ("MAIN_BASELINE", "HEAD_REF", "PR_BASE", "PR_HEAD")}
        run_env.update(MAIN_REF="main", BETA_REF="beta")
        run_env.update({k: v for k, v in env.items() if v is not None})
        for name in [k for k, v in env.items() if v is None]:
            run_env.pop(name, None)
        done = subprocess.run(
            ["bash", SCRIPT, check], cwd=self.dir, env=run_env, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        return done.returncode, done.stdout

    def main_in_beta(self, baseline):
        return self.lines("main-in-beta", HEAD_REF="beta", MAIN_BASELINE=baseline)

    def beta_born(self, head="pr", base="main"):
        return self.lines("beta-born", PR_BASE=self.sha(base), PR_HEAD=self.sha(head))

    # the planted shapes

    def fix_on_beta(self, name="fix.txt", text="the fix\n", fragment=None):
        """A fix born on beta; returns its sha and leaves the checkout on beta."""
        self.on("beta")
        self.write(name, text)
        if fragment:
            self.write(fragment, "- **A fix of the door.**\n")
        return self.commit("beta: a fix in %s" % name)

    def hop(self, toml=True):
        if toml:
            self.edit("Cargo.toml", 'bsv-rs = "=0.8.1"', 'bsv-rs = "=0.8.2"')
        self.edit("Cargo.lock", 'name = "bsv-rs"\nversion = "0.8.1"', 'name = "bsv-rs"\nversion = "0.8.2"')
        return self.commit("deps(stack): bump bsv-rs from 0.8.1 to 0.8.2")

    def version(self, subject="1.0.1", manifest=True, extra=None):
        """A version commit on the current branch, which holds changelog.d/7.md."""
        if manifest:
            self.edit("Cargo.toml", 'version = "1.0.0"', 'version = "1.0.1"')
            self.edit("Cargo.lock", 'name = "app"\nversion = "1.0.0"', 'name = "app"\nversion = "1.0.1"')
            self.git("rm", "-q", "changelog.d/7.md")
        self.edit("CHANGELOG.md", "## [1.0.0]", SECTION + "## [1.0.0]")
        if extra:
            extra()
        return self.commit(subject)

    def pr_with_pick(self):
        """beta holds a fix with its fragment; `pr`, cut from main, holds its pick with -x."""
        fix = self.fix_on_beta(fragment="changelog.d/7.md")
        self.on("pr", "main")
        self.git("cherry-pick", "-x", fix)
        return fix


def expect(status, output, want, *needles):
    """One judged run: a list of the misses."""
    missed = []
    if want == "nonzero":
        if status == 0:
            missed.append("exit 0, wanted non-zero")
    elif status != want:
        missed.append("exit %d, wanted %d" % (status, want))
    for needle in needles:
        if needle not in output:
            missed.append("the output lacks %r" % needle)
    if missed:
        missed.append("output:\n" + output)
    return missed


# main-in-beta

def m1(r):
    return expect(*r.main_in_beta(r.sha("main")), 0, "every commit of main since the baseline")


def m2(r):
    fix = r.fix_on_beta()
    r.on("main")
    r.git("cherry-pick", fix)          # the same patch, no line
    return expect(*r.main_in_beta(r.base), 0)


def m3(r):
    fix = r.fix_on_beta()
    r.on("main")
    r.write("fix.txt", "the fix, as main's tree needed it\n")
    r.commit("main: the fix, resolved by hand\n\n(cherry picked from commit %s)" % fix)
    return expect(*r.main_in_beta(r.base), 0)


def m4(r):
    r.write("born.txt", "born on main\n")
    born = r.commit("main: a commit with no line")
    r.on("beta")
    r.write("born.txt", "the same matter in beta's words\n")
    r.commit("beta: the link\n\n(cherry picked from commit %s)" % born)
    return expect(*r.main_in_beta(r.base), 0)


def m5(r):
    r.hop(toml=False)
    return expect(*r.main_in_beta(r.base), 0, "a hop commit")


def m6(r):
    r.hop()
    return expect(*r.main_in_beta(r.base), 0, "a hop commit")


def m7(r):
    r.edit("Cargo.toml", 'serde = "1.0.1"', 'serde = "1.0.2"')
    r.edit("Cargo.lock", 'name = "serde"\nversion = "1.0.1"', 'name = "serde"\nversion = "1.0.2"')
    r.commit("main: serde moved on main")
    return expect(*r.main_in_beta(r.base), 1, "main: serde moved on main", "::error::")


def m8(r):
    fix = r.fix_on_beta(fragment="changelog.d/7.md")
    r.on("main")
    r.git("cherry-pick", "-x", fix)
    version = r.version("main: the version commit 1.0.1")
    missed = expect(*r.main_in_beta(r.base), 1, "main: the version commit 1.0.1")
    r.on("beta")
    r.edit("CHANGELOG.md", "## [1.0.0]", SECTION + "## [1.0.0]")
    r.commit("link: 1.0.1 into beta\n\n(cherry picked from commit %s)" % version)
    return missed + expect(*r.main_in_beta(r.base), 0)


def m9(r):
    r.write("HANDOFF.md", "The record.\nA row born on main.\n")
    r.commit("HANDOFF: a row born on main")
    return expect(*r.main_in_beta(r.base), 1, "HANDOFF: a row born on main")


def m10(r):
    r.write("old.txt", "born under the old direction\n")
    old = r.commit("main: unlinked, before the baseline")
    fix = r.fix_on_beta()
    r.on("main")
    r.git("cherry-pick", "-x", fix)
    return expect(*r.main_in_beta(old), 0)


def m11(r):
    return expect(*r.main_in_beta(r.sha("beta")), 1, "the baseline")


def m12(r):
    return expect(*r.lines("main-in-beta", HEAD_REF="beta", MAIN_BASELINE=None), "nonzero", "MAIN_BASELINE")


def m13(r):
    r.on("side", r.base)
    r.write("side.txt", "on no line\n")
    side = r.commit("side: on neither line")
    r.on("main")
    r.write("a.txt", "a\n")
    r.commit("main: names a commit of no line\n\n(cherry picked from commit %s)" % side)
    missed = expect(*r.main_in_beta(r.base), 1, "main: names a commit of no line")
    at = r.sha("main")
    r.write("b.txt", "b\n")
    r.commit("main: names an unknown object\n\n(cherry picked from commit %s)" % UNKNOWN)
    return missed + expect(*r.main_in_beta(at), 1, "main: names an unknown object")


# beta-born

def b1(r):
    r.pr_with_pick()
    return expect(*r.beta_born(), 0, "every commit of the pull request is on beta")


def b2(r):
    fix = r.fix_on_beta()
    r.on("pr", "main")
    r.write("fix.txt", "the fix, as main's tree needed it\n")
    r.commit("the fix, resolved by hand\n\n(cherry picked from commit %s)" % fix)
    return expect(*r.beta_born(), 0)


def b3(r):
    r.fix_on_beta()
    r.on("pr", "main")
    r.write("fix.txt", "the fix, as main's tree needed it\n")
    r.commit("the fix, resolved by hand, its line lost")
    return expect(*r.beta_born(), 1, "the fix, resolved by hand, its line lost", "::error::")


def b4(r):
    r.on("pr", "main")
    r.hop()
    return expect(*r.beta_born(), 0, "a hop commit")


def b5(r):
    r.pr_with_pick()
    r.version()
    return expect(*r.beta_born(), 0, "a version commit")


def b6(r):
    r.pr_with_pick()
    r.version("1.0.1 and the front page", extra=lambda: r.write("README.md", "The front page, 1.0.1.\n"))
    return expect(*r.beta_born(), 1, "1.0.1 and the front page")


def b7(r):
    r.pr_with_pick()
    r.version("1.0.1 and serde", extra=lambda: r.edit("Cargo.toml", 'serde = "1.0.1"', 'serde = "1.0.2"'))
    return expect(*r.beta_born(), 1, "1.0.1 and serde")


def b8(r):
    r.pr_with_pick()
    r.version("1.0.1 and the fragments' README", extra=lambda: r.git("rm", "-q", "changelog.d/README.md"))
    return expect(*r.beta_born(), 1, "1.0.1 and the fragments' README")


def b9(r):
    r.on("pr", "main")
    r.version("CHANGELOG.md alone", manifest=False)
    return expect(*r.beta_born(), 1, "CHANGELOG.md alone")


def b10(r):
    r.pr_with_pick()
    r.version("1.0.1 and a line of prose removed", extra=lambda: r.edit("CHANGELOG.md", "- The beginning.\n", ""))
    return expect(*r.beta_born(), 1, "1.0.1 and a line of prose removed")


def b11(r):
    fix = r.fix_on_beta()
    r.on("side", "main")
    r.git("cherry-pick", "-x", fix)
    r.on("pr", "main")
    r.git("merge", "-q", "--no-ff", "-m", "a merge in the pull request", "side")
    return expect(*r.beta_born(), 0, "1 merge commit(s)")


def b12(r):
    r.pr_with_pick()
    status, output = r.lines("beta-born", BETA_REF="no-such-line", PR_BASE=r.sha("main"), PR_HEAD=r.sha("pr"))
    return expect(status, output, "nonzero")


def b13(r):
    return expect(*r.beta_born(head="main"), 0)


CASES = [
    ("M1", "main at the baseline", m1),
    ("M2", "a main commit with the same patch as a beta commit", m2),
    ("M3", "a main commit whose patch differs, its message naming a beta commit", m3),
    ("M4", "a main commit with no line; a beta commit naming its sha", m4),
    ("M5", "a main commit changing Cargo.lock alone", m5),
    ("M6", "Cargo.toml's bsv-rs line and Cargo.lock", m6),
    ("M7", "Cargo.toml's serde line and Cargo.lock", m7),
    ("M8", "a version commit, unlinked; then a beta commit naming it", m8),
    ("M9", "a main commit changing HANDOFF.md alone", m9),
    ("M10", "an unlinked main commit before the baseline", m10),
    ("M11", "a baseline that is not on main", m11),
    ("M12", "MAIN_BASELINE unset", m12),
    ("M13", "a message naming a sha on no line; one naming an unknown object", m13),
    ("B1", "one clean cherry-pick -x", b1),
    ("B2", "a pick resolved by hand, the line kept", b2),
    ("B3", "the same without the line", b3),
    ("B4", "a hop commit", b4),
    ("B5", "a version commit", b5),
    ("B6", "B5 with README.md changed too", b6),
    ("B7", "B5 with a dependency line changed in Cargo.toml", b7),
    ("B8", "B5 deleting changelog.d/README.md", b8),
    ("B9", "CHANGELOG.md alone", b9),
    ("B10", "B5 removing a prose line of CHANGELOG.md", b10),
    ("B11", "a merge commit in the range beside a clean pick", b11),
    ("B12", "BETA_REF naming no ref", b12),
    ("B13", "an empty range", b13),
]


def main():
    root = tempfile.mkdtemp(prefix="lines-")
    failed = 0
    try:
        for name, planted, case in CASES:
            try:
                missed = case(Repo(root, name))
            except Exception as err:  # a case that could not be planted is a miss, by name
                missed = ["not planted: %s" % err]
            if missed:
                failed += 1
                print("FAIL %s (%s)" % (name, planted))
                for line in missed:
                    print("  " + line.replace("\n", "\n  "))
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print("lines: %d cases" % len(CASES))
    if failed:
        print("lines: %d failed" % failed)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
