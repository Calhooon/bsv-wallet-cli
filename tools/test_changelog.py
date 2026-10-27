#!/usr/bin/env python3
"""Planted cases for tools/changelog (#195), each in its own git repository
under $TMPDIR, with a base and a head commit. No network or bytecode in the
checkout. Run by the CI job ratchet: python3 -B tools/test_changelog.py

The simple heading rule deliberately refuses some lines that CommonMark
would not render as headings: "    # x", "#hashtag", "- - -", "Foo\n- - -",
and "# comment" inside a fenced block. Indentation and fences never exempt
a line. The retired test_a_plain_rule_after_a_blank_line_is_allowed is now
test_dash_rule_after_blank_line_is_refused_by_name; use "***" for a rule.
"""

from datetime import datetime, timezone
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
TOOL = Path(__file__).resolve().with_name("changelog")
BODY = ("\n### Beta.1: the pins together (#140)\n\n"
        "A pin paragraph with **words** and a second line.  \n"
        "The next line keeps its spaces and café.\n\n"
        "- An existing bullet.\n\n- The last existing bullet.\n\n")
HISTORY = "## [0.18.2] - 2026-10-26\n\nReleased prose.\n\n- Released bullet.\n"
CHANGELOG = "# Changelog\n\n## [Unreleased]\n" + BODY + HISTORY
FRAGMENTS = {"2.md": "- **Two.** A change.\n", "10.md": "A paragraph\non two lines.\n"}


class Tree:
    def __init__(self):
        self.root = Path(tempfile.mkdtemp(prefix="changelog-case.", dir=os.environ.get("TMPDIR")))
        self.git("init", "-q", "-b", "base")
        self.write("tools/changelog", TOOL.read_bytes())
        self.write("changelog.d/README.md", "# Fragments\n")
        self.write("CHANGELOG.md", CHANGELOG)
        self.write("src/lib.rs", "// base\n")
        self.write("Cargo.toml", "# base\n")
        self.base = self.commit("base")

    def cleanup(self):
        shutil.rmtree(self.root)

    def git(self, *args):
        env = dict(os.environ, GIT_AUTHOR_NAME="Changelog test", GIT_COMMITTER_NAME="Changelog test",
                   GIT_AUTHOR_EMAIL="test@example.invalid", GIT_COMMITTER_EMAIL="test@example.invalid")
        result = subprocess.run(["git", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                                 "-c", "core.autocrlf=false", *args], cwd=self.root, env=env,
                                capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        return result.stdout.strip()

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content.encode("utf-8") if isinstance(content, str) else content)

    def commit(self, message="head"):
        self.git("add", "--all")
        self.git("commit", "-q", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    def run(self, *args):
        result = subprocess.run([sys.executable, "-B", self.root / "tools/changelog", *args],
                                cwd=self.root, capture_output=True, text=True)
        return result.returncode, (result.stdout + result.stderr).strip()

    def snapshot(self):
        files = {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*")
                 if ".git" not in p.relative_to(self.root).parts and p.is_file()}
        return files, self.git("status", "--porcelain")


class Case(unittest.TestCase):
    def setUp(self):
        self.tree = Tree()
        self.addCleanup(self.tree.cleanup)

    def result(self, code, needle, *args, readonly=True):
        before = self.tree.snapshot()
        actual, out = self.tree.run(*args)
        self.assertEqual(actual, code, out)
        self.assertIn(needle, out)
        if readonly:
            self.assertEqual(self.tree.snapshot(), before, out)
        print(f"\n    exit {code}: {out.splitlines()[-1] if out else '(no output)'}",
              end=" ", file=sys.stderr)
        return out

    def check(self, code, needle, *extra):
        return self.result(code, needle, "--check", "--base", self.tree.base, *extra)

    def assemble(self, version="0.19.0", code=0, needle="assembled", *extra):
        return self.result(code, needle, "--assemble", version, "--date", "2026-10-26", *extra,
                           readonly=code != 0)


class WatchedChanges(Case):
    def test_src_without_a_fragment_fails_naming_the_path(self):
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.check(1, "src/lib.rs")

    def test_src_with_an_added_fragment_passes(self):
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.check(0, "added fragment: changelog.d/195.md")

    def test_allow_missing_passes_and_names_the_label(self):
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        out = self.check(0, "no-changelog label allowed missing fragment", "--allow-missing")
        self.assertIn("src/lib.rs", out)

    def test_docs_without_a_fragment_passes(self):
        self.tree.write("docs/note.md", "A note.\n")
        self.tree.commit()
        self.check(0, "no watched paths changed")

    def test_cargo_without_a_fragment_fails(self):
        self.tree.write("Cargo.toml", "# changed\n")
        self.tree.commit()
        self.check(1, "Cargo.toml")

    def test_mcp_and_e2e_are_watched_and_every_path_is_named(self):
        self.tree.write("mcp/src/main.rs", "// changed\n")
        self.tree.write("tests/e2e/run.js", "// changed\n")
        self.tree.commit()
        out = self.check(1, "mcp/src/main.rs")
        self.assertIn("tests/e2e/run.js", out)

    def test_similar_directory_names_are_not_watched(self):
        for path in ("src-extra/lib.rs", "mcp/src-extra/lib.rs", "tests/e2e-extra/run.js", "sub/Cargo.toml"):
            self.tree.write(path, "A change.\n")
        self.tree.commit()
        self.check(0, "no watched paths changed")

    def test_deleting_a_fragment_does_not_count_as_adding_one(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.base = self.tree.commit("base with fragment")
        (self.tree.root / "changelog.d/195.md").unlink()
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.check(1, "src/lib.rs")

    def test_editing_a_fragment_does_not_count_as_adding_one(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.base = self.tree.commit("base with fragment")
        self.tree.write("changelog.d/195.md", "- Another change.\n")
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.check(1, "src/lib.rs")

    def test_an_added_readme_does_not_count_as_a_fragment(self):
        (self.tree.root / "changelog.d/README.md").unlink()
        self.tree.base = self.tree.commit("base without readme")
        self.tree.write("changelog.d/README.md", "# Fragments\n")
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.check(1, "src/lib.rs")

    def test_a_deleted_watched_path_needs_a_fragment_too(self):
        (self.tree.root / "src/lib.rs").unlink()
        self.tree.commit()
        self.check(1, "src/lib.rs")

    def test_src_moved_out_needs_a_fragment_with_rename_detection_enabled(self):
        self.tree.git("config", "diff.renames", "true")
        (self.tree.root / "lib").mkdir()
        self.tree.git("mv", "src/lib.rs", "lib/lib.rs")
        self.tree.commit()
        self.assertIn("R100", self.tree.git("diff", "--name-status", self.tree.base))
        self.check(1, "src/lib.rs")

    def test_similar_added_fragment_counts_beside_a_deleted_one_with_renames_enabled(self):
        self.tree.git("config", "diff.renames", "true")
        shared = "- Keep pending rows visible until the refreshed balance is ready.\n" * 4
        self.tree.write("changelog.d/100.md", shared + "- Fix an old balance refresh.\n")
        self.tree.base = self.tree.commit("base with fragment")
        (self.tree.root / "changelog.d/100.md").unlink()
        self.tree.write("changelog.d/195.md", shared + "- Fix a new balance refresh.\n")
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.assertIn("R", self.tree.git("diff", "--name-status", self.tree.base))
        self.check(0, "added fragment: changelog.d/195.md")

    def copy_fragment(self, content, renames="true"):
        self.tree.git("config", "diff.renames", renames)
        self.tree.write("changelog.d/10.md", "- Change A.\n")
        self.tree.base = self.tree.commit("base with fragment")
        self.tree.write("changelog.d/11.md", content)
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()

    def test_identical_fragment_copy_does_not_count_as_a_new_entry(self):
        self.copy_fragment("- Change A.\n")
        self.assertIn("C100\tchangelog.d/10.md\tchangelog.d/11.md",
                      self.tree.git("diff", "--name-status", "-C", "-C", self.tree.base))
        out = self.check(1, "changelog.d/11.md repeats changelog.d/10.md byte for byte; "
                         "write the change's own entry")
        self.assertIn("src/lib.rs", out)

    def test_fragment_copy_with_one_byte_changed_counts_as_a_new_entry(self):
        self.copy_fragment("- Change B.\n")
        self.check(0, "added fragment: changelog.d/11.md")

    def test_identical_fragment_copy_does_not_count_with_copy_detection_enabled(self):
        self.copy_fragment("- Change A.\n", renames="copies")
        self.assertIn("C100\tchangelog.d/10.md\tchangelog.d/11.md",
                      self.tree.git("diff", "--name-status", "-C", "-C", self.tree.base))
        out = self.check(1, "changelog.d/11.md repeats changelog.d/10.md byte for byte; "
                         "write the change's own entry")
        self.assertIn("src/lib.rs", out)

    def test_renamed_fragment_is_not_new_with_rename_detection_enabled(self):
        self.tree.git("config", "diff.renames", "true")
        self.tree.write("changelog.d/100.md", "- An existing change.\n")
        self.tree.base = self.tree.commit("base with fragment")
        self.tree.git("mv", "changelog.d/100.md", "changelog.d/195.md")
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.assertIn("R100", self.tree.git("diff", "--name-status", self.tree.base))
        out = self.check(1, "changelog.d/195.md repeats changelog.d/100.md byte for byte; "
                         "write the change's own entry")
        self.assertIn("src/lib.rs", out)

    def test_an_unknown_base_is_exit_2_even_with_the_label(self):
        self.tree.commit()
        self.result(2, "cannot read git diff for no-such-revision", "--check",
                    "--base", "no-such-revision", "--allow-missing")

    def test_a_base_cannot_pass_a_writing_option_to_git(self):
        self.tree.write("src/lib.rs", "// changed\n")
        self.tree.commit()
        self.result(2, "--base must be a revision", "--check", "--base=--output=unexpected")

    def test_an_empty_base_is_exit_2(self):
        self.tree.commit()
        self.result(2, "--base must be a revision", "--check", "--base", "")


class Assembly(Case):
    def test_list_orders_by_number_then_name_and_never_writes(self):
        for name in ("10.md", "2-z.md", "2.md", "2-a.md"):
            self.tree.write(f"changelog.d/{name}", "- A change.\n")
        self.tree.commit()
        out = self.result(0, "changelog.d/10.md", "--list")
        self.assertEqual(out.splitlines(), ["changelog.d/2-a.md", "changelog.d/2-z.md",
                                           "changelog.d/2.md", "changelog.d/10.md"])

    def test_two_fragment_prs_merge_in_either_order_and_assemble_identically(self):
        outputs = []
        for order in (("lane-a", "lane-b"), ("lane-b", "lane-a")):
            tree = Tree()
            self.addCleanup(tree.cleanup)
            for branch, name in (("lane-a", "10.md"), ("lane-b", "2.md")):
                tree.git("checkout", "-q", "-b", branch, tree.base)
                tree.write(f"changelog.d/{name}", FRAGMENTS[name])
                tree.commit(branch)
            tree.git("checkout", "-q", "-b", "head", tree.base)
            for branch in order:
                tree.git("merge", "-q", "--no-ff", "--no-edit", branch)
            code, out = tree.run("--assemble", "0.19.0", "--date", "2026-10-26")
            self.assertEqual(code, 0, out)
            outputs.append((tree.root / "CHANGELOG.md").read_bytes())
        self.tree.commit()
        self.assertEqual(outputs[0], outputs[1])
        print("\n    two merge orders: identical assembled bytes", end=" ", file=sys.stderr)

    def test_unreleased_body_stays_verbatim_ahead_of_fragments_and_history(self):
        for name, content in FRAGMENTS.items():
            self.tree.write(f"changelog.d/{name}", content)
        self.tree.commit()
        self.assemble()
        want = ("# Changelog\n\n## [Unreleased]\n\n## [0.19.0] - 2026-10-26\n" + BODY
                + FRAGMENTS["2.md"] + "\n" + FRAGMENTS["10.md"] + "\n" + HISTORY)
        self.assertEqual((self.tree.root / "CHANGELOG.md").read_bytes(), want.encode("utf-8"))
        self.assertEqual([p.name for p in (self.tree.root / "changelog.d").iterdir()], ["README.md"])
        self.assemble(code=1, needle="section for 0.19.0 already exists")

    def test_prerelease_with_build_metadata_is_a_semver(self):
        self.tree.commit()
        self.assemble("0.19.0-beta.1+build.7")
        self.assertIn("## [0.19.0-beta.1+build.7] - 2026-10-26",
                      (self.tree.root / "CHANGELOG.md").read_text())

    def test_prerelease_is_a_semver(self):
        self.tree.commit()
        self.assemble("0.19.0-beta.1")

    def test_only_unreleased_body_is_enough_to_assemble(self):
        self.tree.commit()
        self.assemble()
        self.assertEqual((self.tree.root / "CHANGELOG.md").read_text(),
                         "# Changelog\n\n## [Unreleased]\n\n## [0.19.0] - 2026-10-26\n" + BODY + HISTORY)

    def test_only_fragments_are_enough_to_assemble(self):
        self.tree.write("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n" + HISTORY)
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.assemble()
        self.assertEqual((self.tree.root / "CHANGELOG.md").read_text(),
                         "# Changelog\n\n## [Unreleased]\n\n## [0.19.0] - 2026-10-26\n\n- A change.\n\n" + HISTORY)

    def test_no_unreleased_body_or_fragments_is_refused_without_writes(self):
        self.tree.write("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n" + HISTORY)
        self.tree.commit()
        self.assemble(code=1, needle="nothing to assemble")

    def test_minor_after_prerelease_refuses_empty_assembly_without_the_flag(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.assemble("0.19.0-beta.1")
        self.assemble(code=1, needle="nothing to assemble")

    def test_allow_empty_reads_the_heading_directly_below_after_prerelease(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.assemble("0.27.0-beta.3+build.9")
        previous = (self.tree.root / "CHANGELOG.md").read_text()
        self.assemble("0.27.0", 0, "removed 0 fragments", "--allow-empty")
        self.assertEqual((self.tree.root / "CHANGELOG.md").read_text(), previous.replace(
            "## [Unreleased]\n\n", "## [Unreleased]\n\n## [0.27.0] - 2026-10-26\n\n"
            "No changes since [0.27.0-beta.3+build.9] - 2026-10-26.\n\n", 1))

    def test_allow_empty_without_a_previous_heading_is_refused_without_writes(self):
        self.tree.write("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n")
        self.tree.commit()
        self.assemble("0.19.0", 1, "previous release heading", "--allow-empty")

    def test_allow_empty_goes_only_with_assembly(self):
        self.tree.commit()
        self.result(2, "--allow-empty goes only with --assemble", "--list", "--allow-empty")

    def test_readonly_fragment_directory_refuses_before_writing_anything(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        directory = self.tree.root / "changelog.d"
        original_mode = directory.stat().st_mode
        self.addCleanup(directory.chmod, original_mode)
        directory.chmod(0o555)
        if os.access(directory, os.W_OK):
            self.skipTest("this user can write read-only directories")
        self.assemble(code=1, needle="changelog.d: cannot delete fragments")

    def test_a_stars_rule_after_a_blank_line_is_allowed(self):
        self.tree.write("changelog.d/195.md", "A paragraph.\n\n***\n\nAnother paragraph.\n")
        self.tree.commit()
        self.result(0, "changelog.d/195.md", "--list")
        self.assemble()
        self.assertIn("A paragraph.\n\n***\n\nAnother paragraph.",
                      (self.tree.root / "CHANGELOG.md").read_text())

    def test_existing_version_is_refused_without_writes(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.assemble("0.18.2", 1, "section for 0.18.2 already exists")

    def test_default_date_is_today_in_utc(self):
        self.tree.commit()
        before = datetime.now(timezone.utc).date().isoformat()
        self.result(0, "assembled 0.19.0", "--assemble", "0.19.0", readonly=False)
        after = datetime.now(timezone.utc).date().isoformat()
        text = (self.tree.root / "CHANGELOG.md").read_text()
        self.assertTrue(any(f"## [0.19.0] - {day}\n" in text for day in (before, after)), text)

    def test_invalid_calendar_date_is_refused_without_writes(self):
        self.tree.commit()
        self.result(1, "invalid date", "--assemble", "0.19.0", "--date", "2026-02-30")

    def test_non_iso_date_is_refused_without_writes(self):
        self.tree.commit()
        self.result(1, "date must be YYYY-MM-DD", "--assemble", "0.19.0", "--date", "2026-1-2")

    def test_empty_date_is_refused_without_writes(self):
        self.tree.commit()
        self.result(1, "date must be YYYY-MM-DD", "--assemble", "0.19.0", "--date", "")


class Malformed(Case):
    def malformed(self, name, content):
        self.tree.write(f"changelog.d/{name}", content)
        self.tree.commit()
        for args in (("--list",), ("--check", "--base", self.tree.base),
                     ("--check", "--base", self.tree.base, "--allow-missing"),
                     ("--assemble", "0.19.0")):
            self.result(1, f"changelog.d/{name.split('/')[0]}", *args)

    def test_a_subdirectory_is_refused(self):
        self.malformed("195.md/nested.md", "- Nested.\n")


def malformed_case(name, content):
    def test(self):
        self.malformed(name, content)
    return test


for case, name, content in (
        ("missing_number", "entry.md", "- A change.\n"),
        ("missing_slug", "195-.md", "- A change.\n"),
        ("bad_slug", "195-two words.md", "- A change.\n"),
        ("wrong_extension", "195.txt", "- A change.\n"),
        ("extra_extension", "195.md.bak", "- A change.\n"),
        ("empty", "195.md", b""),
        ("whitespace", "195.md", " \t\n"),
        ("heading", "195.md", "- A change.\n\n## Release\n"),
        ("indented_heading", "195.md", "   ### Release\n"),
        ("setext_dash_heading", "195.md", "Foo\n---\n"),
        ("setext_equals_heading", "195.md", "Foo\n===\n"),
        ("single_dash_setext_heading", "195.md", "Foo\n-\n"),
        ("single_equals_setext_heading", "195.md", "Foo\n=\n"),
        ("standalone_dash_rule", "195.md", "---\n"),
        ("standalone_equals_rule", "195.md", "===\n"),
        ("dash_rule_after_blank_line", "195.md", "A paragraph.\n\n---\n\nAnother paragraph.\n"),
        ("equals_rule_after_blank_line", "195.md", "Foo\n\n===\n"),
        ("spaced_dash_rule", "195.md", "- - -\n"),
        ("spaced_dash_rule_after_text", "195.md", "Foo\n- - -\n"),
        ("spaced_equals_rule", "195.md", "= = =\n"),
        ("single_dash_with_trailing_whitespace", "195.md", "- \t\n"),
        ("list_item_heading", "195.md", "- ## Release\n"),
        ("star_list_item_heading", "195.md", "  * ### Release\n"),
        ("plus_list_item_heading", "195.md", "+ # Release\n"),
        ("ordered_dot_heading", "195.md", "1. # x\n"),
        ("ordered_paren_heading", "195.md", "1) # x\n"),
        ("nine_digit_ordered_heading", "195.md", "123456789) # x\n"),
        ("blockquote_heading", "195.md", "> # x\n"),
        ("nested_blockquote_heading", "195.md", "> > # x\n"),
        ("compact_blockquote_heading", "195.md", ">># x\n"),
        ("blockquote_list_heading", "195.md", "> - # x\n"),
        ("list_blockquote_heading", "195.md", "- > # x\n"),
        ("indented_containers_heading", "195.md", "    >     -     1) # x\n"),
        ("blockquote_setext_heading", "195.md", "> Foo\n> ---\n"),
        ("nested_list_heading", "195.md", "- - # x\n"),
        ("nested_bullet_heading", "195.md", "- x\n  - # y\n"),
        ("bullet_continuation_heading", "195.md", "- x\n    # y\n"),
        ("ordered_continuation_heading", "195.md", "1. x\n    # y\n"),
        ("bullet_continuation_setext_heading", "195.md", "- foo\n    ---\n"),
        ("tab_continuation_setext_heading", "195.md", "- Foo\n\t---\n"),
        ("four_space_indented_heading", "195.md", "    # x\n"),
        ("tab_indented_heading", "195.md", "\t# x\n"),
        ("line_start_hashtag", "195.md", "#hashtag\n"),
        ("fenced_comment", "195.md", "```\n# comment\n```\n"),
        ("empty_list_heading_at_eof", "195.md", "- #"),
        ("non_utf8", "195.md", b"- Invalid \xff\n")):
    setattr(Malformed, f"test_{case}_is_refused_by_name", malformed_case(name, content))


def allowed_fragment_case(content):
    def test(self):
        self.tree.write("changelog.d/195.md", content)
        self.tree.commit()
        self.result(0, "changelog.d/195.md", "--list")
        self.assemble()
        self.assertIn(content, (self.tree.root / "CHANGELOG.md").read_text())
    return test


for case, content in (("stars_rule_after_text", "Foo\n***\n"),
                      ("standalone_stars_rule", "***\n"),
                      ("ordinary_bold_bullet", "- **bold**: text\n"),
                      ("negative_number_in_prose", "-1 sats\n"),
                      ("decimal_number_in_prose", "1.5 BSV\n"),
                      ("ten_digit_number_prefix", "1234567890. # x\n"),
                      ("inline_hashtag", "A #hashtag inside a line.\n"),
                      ("inline_backticks", "Use `# comment` inline.\n"),
                      ("tab_indented_bullet_continuation", "- A change.\n\tContinued text.\n"),
                      ("url_fragment", "https://example.invalid/path#fragment\n")):
    setattr(Assembly, f"test_{case}_is_allowed", allowed_fragment_case(content))


def invalid_version_case(version):
    def test(self):
        self.tree.write("changelog.d/195.md", "- A change.\n")
        self.tree.commit()
        self.assemble(version, 1, "not a semantic version")
    return test


for case, version in (("word", "latest"), ("partial", "0.19"), ("v_prefix", "v0.19.0"),
                      ("leading_zero", "00.19.0"), ("prerelease_zero", "0.19.0-beta.01"),
                      ("empty_prerelease", "0.19.0-"), ("empty_identifier", "0.19.0-beta..1")):
    setattr(Assembly, f"test_invalid_version_{case}_writes_nothing", invalid_version_case(version))


if __name__ == "__main__":
    unittest.main(verbosity=2)
