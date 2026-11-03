"""A test file's template git repository, made once and copied into each case
(#525), with git's background maintenance off and its transient files left
behind by the copy (#538).

After a commit, git may start a detached `gc --auto` or `maintenance run
--auto` in the repository; it writes and removes `.git/objects/maintenance.lock`
(and pack temporaries), and a case's copytree that listed the file before it
vanished fails on it. quiet() sets gc.auto=0 and maintenance.auto=false in the
repository's own config right after `git init`, before any command writes an
object, so no later git command there (the template's or a copy's, which
inherits the config) starts one. copy() skips git's transient names under
`.git` alone (a working-tree `Cargo.lock` is copied), so a file another
process holds there for a moment is never part of a case's listing.
"""

import fnmatch
import os
import shutil
import subprocess
import tempfile
import threading

QUIET = (("gc.auto", "0"), ("maintenance.auto", "false"))
TRANSIENT = ("*.lock", "tmp_*", ".tmp-*", "gc.pid")


def quiet(root):
    """Turn background maintenance off in the repository at `root`."""
    for key, value in QUIET:
        subprocess.run(["git", "config", key, value], cwd=root, check=True, capture_output=True)


def transient(template):
    """copytree's ignore for `template`: git's transient names in a directory
    under its `.git` (the path is read relative to the template, so a $TMPDIR
    with a `.git` in it changes nothing)."""
    def ignore(directory, names):
        if ".git" not in os.path.relpath(directory, template).split(os.sep):
            return set()
        return {n for n in names if any(fnmatch.fnmatchcase(n, p) for p in TRANSIENT)}
    return ignore


def copy(template, root, **kwargs):
    """The template's tree copied into `root`, as each case copies it."""
    shutil.copytree(template, root, symlinks=True, dirs_exist_ok=True, ignore=transient(template), **kwargs)


class PlantedCopies:
    """The #538 plant, mixed into a test file's unittest.TestCase that defines
    template_root(): its own template, with a maintenance.lock planted in it."""

    def template_root(self):
        raise NotImplementedError

    def lock(self):
        return os.path.join(self.template_root(), ".git", "objects", "maintenance.lock")

    def target(self):
        target = tempfile.mkdtemp(prefix="git-template-plant.", dir=os.environ.get("TMPDIR"))
        self.addCleanup(shutil.rmtree, target, True)
        return target

    def test_the_template_and_its_copy_have_maintenance_off(self):
        target = self.target()
        copy(self.template_root(), target)
        for root in (self.template_root(), target):
            for key, value in QUIET:
                got = subprocess.run(["git", "config", "--get", key], cwd=root,
                                     capture_output=True, text=True).stdout.strip()
                self.assertEqual(got, value, (root, key))

    def test_a_lock_removed_between_the_listing_and_its_copy_cannot_fail_the_copy(self):
        """Deterministic: the lock is in the template when copytree lists
        `.git/objects` and is gone before its own copy, as on run 41652494377."""
        lock = self.lock()
        self.addCleanup(lambda: os.path.exists(lock) and os.remove(lock))

        def vanishing(src, dst):  # maintenance ends between copytree's listing and this copy
            if os.path.samefile(os.path.dirname(src), os.path.dirname(lock)) and os.path.exists(lock):
                os.remove(lock)
            return shutil.copy2(src, dst)

        with open(lock, "w"):
            pass
        with self.assertRaises(shutil.Error) as raised:  # the fault, without the ignore
            shutil.copytree(self.template_root(), self.target(), symlinks=True,
                            dirs_exist_ok=True, copy_function=vanishing)
        self.assertIn("maintenance.lock", str(raised.exception))
        with open(lock, "w"):
            pass
        target = self.target()
        copy(self.template_root(), target, copy_function=vanishing)
        self.assertFalse(os.path.exists(os.path.join(target, ".git", "objects", "maintenance.lock")))
        status = [subprocess.run(["git", "status", "--porcelain"], cwd=root, capture_output=True,
                                 text=True, check=True).stdout for root in (self.template_root(), target)]
        self.assertEqual(status[1], status[0])  # the copy is the template's repository

    def test_a_thread_writing_and_removing_the_lock_during_copies_cannot_fail_them(self):
        lock, stop, cycles = self.lock(), threading.Event(), [0]

        def maintenance():
            while not stop.is_set():
                with open(lock, "w") as f:
                    f.write("pid\n")
                os.remove(lock)
                cycles[0] += 1

        thread = threading.Thread(target=maintenance)
        thread.start()
        try:
            for _ in range(20):
                copy(self.template_root(), self.target())
        finally:
            stop.set()
            thread.join()
        self.assertGreater(cycles[0], 0)
        self.assertFalse(os.path.exists(lock))
