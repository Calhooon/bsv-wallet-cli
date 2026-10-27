#!/usr/bin/env python3
"""Planted cases for check-cites (#184, #210, #243), run by CI's ratchet job.

Each case runs the command in its own git repository under $TMPDIR, with a
commit pin and a changed HEAD. No network, cargo, or project fixtures are used.
The process's exit status proves that a wrong citation or count fails the gate.
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True
TOOL = Path(__file__).with_name("check-cites")
SOURCE = "pub fn alpha() {}\n\npub fn beta() {}\n"


class PinnedCitations(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="check-cites-case.", dir=os.environ.get("TMPDIR"))
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / "tools").mkdir()
        shutil.copy(TOOL, self.root / "tools/check-cites")
        self.git("init", "-q")
        self.write("src/lib.rs", SOURCE)
        self.write("src/main.rs", SOURCE)
        self.write("src/name.rs", SOURCE)
        self.write("src/present.rs", SOURCE)
        self.write("tests/integration.rs", SOURCE)
        self.write("tests/present/example.rs", SOURCE)
        self.write("mcp/src/main.rs", "pub fn nested_alpha() {}\n\npub fn nested_beta() {}\n")
        self.git("add", "src", "tests", "mcp")
        self.git("commit", "-qm", "pin")
        self.pin = self.git("rev-parse", "--short=12", "HEAD").strip()
        self.write("src/lib.rs", "// moved at HEAD\n" + SOURCE + "\n")
        self.git("commit", "-qam", "move the source after the pin")
        self.head = self.git("rev-parse", "--short=12", "HEAD").strip()

    def git(self, *args):
        return subprocess.run(["git", "-c", "user.name=Check Cites", "-c",
                               "user.email=check-cites@example.invalid", "-c",
                               "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", *args],
                              cwd=self.root, check=True, capture_output=True, text=True).stdout

    def write(self, name, text):
        p = self.root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def document(self, body="`alpha` (`src/lib.rs:1`).", count="3", header=None):
        if header is None:
            header = f"Line numbers are those of `{self.pin}`."
        return (f"# Architecture\n\n{header}\n\n## Behaviour\n\n{body}\n\n"
                f"| file | lines | purpose | machine |\n|---|---|---|---|\n"
                f"| `src/lib.rs` | {count} | sample | plumbing |\n")

    def run_tool(self, text, *args, name="docs/ARCHITECTURE.md"):
        self.write(name, text)
        return subprocess.run([sys.executable, "-B", str(self.root / "tools/check-cites"), *args],
                              cwd=self.root, capture_output=True, text=True)

    def passes(self, **kwargs):
        result = self.run_tool(self.document(**kwargs))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0 findings", result.stdout)
        return result.stdout

    def fails(self, needle, **kwargs):
        result = self.run_tool(self.document(**kwargs))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(needle, result.stdout)
        self.assertRegex(result.stdout, r"docs/ARCHITECTURE.md:\d+:")
        return result.stdout

    def test_correct_citation_and_count_use_the_pin_despite_head_drift(self):
        out = self.passes()
        self.assertIn("1 citations, 1 line-count rows checked", out)

    def test_wrong_line_fails_the_process(self):
        self.fails("identifier `alpha` is absent", body="`alpha` (`src/lib.rs:3`).")

    def test_wrong_tests_citation_fails_the_process(self):
        self.passes(body="`alpha` (`tests/integration.rs:1`).")
        self.fails("tests/integration.rs:3: identifier `alpha` is absent",
                   body="`alpha` (`tests/integration.rs:3`).")

    def test_mcp_src_citation_uses_the_full_repository_path(self):
        self.passes(body="`nested_alpha` (`mcp/src/main.rs:1`).")
        self.fails("mcp/src/main.rs:1: identifier `alpha` is absent",
                   body="`alpha` (`mcp/src/main.rs:1`).")

    def test_bare_rust_citation_requires_repository_root(self):
        for span in ["name.rs:1", "name.rs:1-3"]:
            with self.subTest(span=span):
                self.fails(f"docs/ARCHITECTURE.md:7: {span}: write the path from the repository root",
                           body=f"`alpha` (`{span}`).")

    def test_relative_rust_citation_requires_repository_root(self):
        for span in ["dir/name.rs:1", "dir/name.rs:1-3"]:
            with self.subTest(span=span):
                self.fails(f"docs/ARCHITECTURE.md:7: {span}: write the path from the repository root",
                           body=f"`alpha` (`{span}`).")

    def test_rooted_rust_citation_passes(self):
        self.passes(body="`alpha` (`src/name.rs:1`).")

    def test_bare_rust_column_requires_repository_root(self):
        self.fails("handlers.rs:2:1: write the path from the repository root",
                   body="`alpha` (`handlers.rs:2:1`).")

    def test_rooted_rust_column_requires_accepted_form(self):
        self.fails("src/handlers.rs:1:5: cite as path:N or path:N-M",
                   body="`alpha` (`src/handlers.rs:1:5`).")

    def test_bare_rust_path_list_requires_repository_root(self):
        self.fails("handlers.rs:1, mod.rs:2: write the path from the repository root",
                   body="`alpha` (`handlers.rs:1, mod.rs:2`).")

    def test_rooted_rust_path_list_requires_accepted_form(self):
        self.fails("src/handlers.rs:1, src/mod.rs:2: cite as path:N or path:N-M",
                   body="`alpha` (`src/handlers.rs:1, src/mod.rs:2`).")

    def test_bare_rust_anchor_requires_repository_root(self):
        self.fails("handlers.rs#L25: write the path from the repository root",
                   body="`alpha` (`handlers.rs#L25`).")

    def test_rooted_rust_anchor_requires_accepted_form(self):
        self.fails("src/handlers.rs#L25: cite as path:N or path:N-M",
                   body="`alpha` (`src/handlers.rs#L25`).")

    def test_nested_crate_line_three_passes(self):
        self.passes(body="`nested_beta` (`mcp/src/main.rs:3`).")

    def test_rooted_rust_range_passes(self):
        self.passes(body="`alpha` (`src/present.rs:1-2`).")

    def test_unnumbered_absent_file_is_named(self):
        self.fails("docs/ARCHITECTURE.md:7: src/absent.rs does not exist at pin",
                   body="The module is `src/absent.rs`.")

    def test_unnumbered_absent_directory_is_named(self):
        self.fails("docs/ARCHITECTURE.md:7: tests/absent/ does not exist at pin",
                   body="The tests are in `tests/absent/`.")

    def test_unnumbered_present_file_passes(self):
        self.passes(body="The module is `src/present.rs`.")

    def test_unnumbered_present_directory_passes(self):
        self.passes(body="The tests are in `tests/present/`.")

    def test_unnumbered_source_root_directory_passes(self):
        self.passes(body="The modules are in `src/`.")

    def test_unnumbered_nested_crate_paths_use_the_repository_root(self):
        self.passes(body="The companion has `mcp/src/main.rs` in `mcp/src/`.")
        self.fails("mcp/src/absent.rs does not exist at pin", body="`mcp/src/absent.rs`.")
        self.fails("mcp/src/absent/ does not exist at pin", body="`mcp/src/absent/`.")

    def test_scheme_less_url_is_not_a_repository_path(self):
        self.passes(body="`github.com/Calhooon/bsv-wallet-cli/src/`.")

    def test_sibling_repository_path_is_not_checked(self):
        self.passes(body="`bsv-rs/src/wallet/interface.rs`.")

    def test_nested_crate_prefix_added_only_at_head_is_not_checked(self):
        self.write("other/src/main.rs", SOURCE)
        self.git("add", "other")
        self.git("commit", "-qm", "add a crate after the pin")
        self.passes(body="`other/src/absent.rs`.")

    def test_non_tree_nested_crate_prefix_is_not_checked(self):
        self.write("other", "not a directory\n")
        self.git("add", "other")
        self.git("commit", "-qm", "add a blob at the prefix")
        self.pin = self.git("rev-parse", "--short=12", "HEAD").strip()
        self.passes(body="`other/src/absent.rs`.", count="5")

    def test_nested_crate_prefix_can_have_multiple_segments(self):
        self.write("group/mcp/src/main.rs", SOURCE)
        self.git("add", "group")
        self.git("commit", "-qm", "add a nested crate")
        self.pin = self.git("rev-parse", "--short=12", "HEAD").strip()
        self.passes(body="`group/mcp/src/main.rs`.", count="5")
        self.fails("group/mcp/src/absent.rs does not exist at pin",
                   body="`group/mcp/src/absent.rs`.", count="5")

    def test_nested_crate_prefixes_are_checked_once_across_documents(self):
        body = ("`mcp/src/main.rs` and `mcp/src/`. "
                "`bsv-rs/src/wallet/interface.rs` and `bsv-rs/src/`.")
        text = self.document(body=body)
        self.write("docs/a.md", text)
        trace = self.root / "git-trace"
        with patch.dict(os.environ, {"GIT_TRACE": str(trace)}):
            result = self.run_tool(text, "docs/a.md", "docs/b.md", name="docs/b.md")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = [line.split("built-in: git ", 1)[-1]
                    for line in trace.read_text().splitlines()]
        pin = self.git("rev-parse", self.pin).strip()
        for prefix in ("mcp/", "bsv-rs/"):
            self.assertEqual(commands.count(f"ls-tree {pin} {prefix}"), 1, commands)

    def test_unnumbered_file_added_only_at_head_still_fails(self):
        self.write("src/absent.rs", SOURCE)
        self.git("add", "src/absent.rs")
        self.git("commit", "-qm", "add a module after the pin")
        self.fails("src/absent.rs does not exist at pin", body="`src/absent.rs`.")

    def test_unnumbered_directory_added_only_at_head_still_fails(self):
        self.write("tests/absent/example.rs", SOURCE)
        self.git("add", "tests/absent")
        self.git("commit", "-qm", "add a test directory after the pin")
        self.fails("tests/absent/ does not exist at pin", body="`tests/absent/`.")

    def test_unnumbered_file_requires_a_blob(self):
        self.fails("tests/present does not exist at pin", body="The file is `tests/present`.")

    def test_unnumbered_directory_requires_a_tree(self):
        self.fails("src/present.rs/ does not exist at pin", body="The directory is `src/present.rs/`.")

    def test_later_marks_an_unnumbered_absent_file(self):
        self.passes(body="The later #12 `src/absent.rs` holds the module.")

    def test_after_the_pin_marks_an_unnumbered_absent_file(self):
        self.passes(body="The module `src/absent.rs` was added after the pin.")

    def test_later_marks_an_unnumbered_absent_directory(self):
        self.passes(body="The later tests live in `tests/absent/`.")

    def test_marker_in_another_sentence_does_not_rescue_a_path(self):
        for marker in ["That module comes later", "That module was added after the pin"]:
            for body in [f"{marker}. The module is `src/absent.rs`.",
                         f"The module is `src/absent.rs`. {marker}."]:
                with self.subTest(body=body):
                    self.fails("src/absent.rs does not exist at pin", body=body)

    def test_marker_in_another_table_cell_does_not_rescue_a_path(self):
        self.fails("src/absent.rs does not exist at pin",
                   body="| The later module | `src/absent.rs` |")

    def test_marker_in_the_same_table_cell_allows_a_path(self):
        self.passes(body="| The later module `src/absent.rs` | The tests |")

    def test_marker_in_another_paragraph_does_not_rescue_a_path(self):
        self.fails("src/absent.rs does not exist at pin",
                   body="The later module\n\n`src/absent.rs`.")

    def test_later_marker_must_be_a_word(self):
        self.fails("src/absent.rs does not exist at pin",
                   body="The collateral module is `src/absent.rs`.")

    def test_marker_does_not_exempt_numbered_citations(self):
        self.fails("name.rs:1: write the path from the repository root",
                   body="The later `alpha` (`name.rs:1`).")
        self.fails("src/absent.rs does not exist at pin",
                   body="The later `alpha` (`src/absent.rs:1`).")

    def test_unnumbered_path_inside_a_fence_is_skipped(self):
        self.passes(body="```rust\n`src/absent.rs`\n```\n"
                         "~~~text\n`tests/absent/`\n~~~")

    def test_rust_citation_inside_a_mermaid_fence_is_skipped(self):
        self.passes(body='```mermaid\nflowchart TD\n'
                         'A["`name.rs:1`"] --> B["`dir/name.rs:1-3`"]\n```')

    def test_count_off_by_one_fails_the_process(self):
        self.fails("line count 4, but pin", count="4")

    def test_a_range_can_hold_the_identifier(self):
        self.passes(body="`module::beta` (`src/lib.rs:2-3`).")

    def test_only_a_whole_identifier_matches(self):
        self.fails("identifier `alp` is absent", body="`alp` (`src/lib.rs:1`).")

    def test_nearest_identifier_wins(self):
        self.fails("identifier `beta` is absent", body="`alpha`, then `beta` (`src/lib.rs:1`).")

    def test_wrapping_does_not_end_a_sentence(self):
        self.passes(body="`alpha` is declared here\n(`src/lib.rs:1`).")

    def test_a_citation_needs_a_preceding_name_in_its_own_context(self):
        for body in ["`src/lib.rs:1` (`alpha`).", "`alpha`. `src/lib.rs:1`.",
                     "`alpha`\n\n`src/lib.rs:1`", "| `alpha` | `src/lib.rs:1` |"]:
            with self.subTest(body=body):
                self.fails("no preceding backticked Rust identifier", body=body)

    def test_sentences_and_cells_each_keep_their_own_names(self):
        self.passes(body="`alpha` (`src/lib.rs:1`). `beta` (`src/lib.rs:3`).\n"
                          "| `alpha` (`src/lib.rs:1`) | `beta` (`src/lib.rs:3`) |")

    def test_shorthand_uses_the_path_in_its_sentence_or_cell(self):
        self.passes(body="`alpha` (`src/lib.rs:1`), `beta` (`:2-3`).")
        self.fails("no preceding source path", body="`alpha` (`src/lib.rs:1`). `beta` (`:3`).")
        self.fails("no preceding source path", body="| `alpha` (`src/lib.rs:1`) | `beta` (`:3`) |")

    def test_missing_file_is_named(self):
        self.fails("src/missing.rs does not exist at pin", body="`alpha` (`src/missing.rs:1`).")

    def test_invalid_and_out_of_bounds_ranges_fail(self):
        for location in ["0", "1-4", "3-2", "100"]:
            with self.subTest(location=location):
                self.fails("invalid range", body=f"`alpha` (`src/lib.rs:{location}`).")

    def test_fences_are_ignored_and_shorter_runs_do_not_close_them(self):
        out = self.passes(body="````rust\n`missing` (`src/missing.rs:100`).\n```\n"
                              "`missing` (`src/missing.rs:100`).\n````\n"
                              "~~~\n`src/missing.rs:100`\n~~~\n`alpha` (`src/lib.rs:1`).")
        self.assertIn("1 citations,", out)

    def test_header_form_alone_and_equivalent_pins_pass(self):
        other = f"At the version at `0.18.0` on `beta` (`{self.pin}`)."
        self.passes(header=other)
        full = self.git("rev-parse", self.pin).strip()
        self.passes(header=other + f"\nLine numbers are those of `{full}`.")

    def test_disagreeing_pins_fail(self):
        self.fails("header pins disagree", header=f"At `0.18.0` (`{self.pin}`).\n"
                   f"Line numbers are those of `{self.head}`.")

    def test_missing_or_unresolvable_pin_fails(self):
        self.fails("no commit pin", header="No pin here.")
        self.fails("does not resolve to a commit", header="Line numbers are those of `0000000`.")

    def test_a_pin_below_the_header_does_not_count(self):
        self.fails("no commit pin", header="No pin here.",
                   body=f"Line numbers are those of `{self.pin}`.")

    def test_a_non_ancestor_pin_fails(self):
        tree = self.git("rev-parse", "HEAD^{tree}").strip()
        unrelated = self.git("commit-tree", tree, "-m", "unrelated pin").strip()
        self.fails("is not an ancestor of HEAD", header=f"Line numbers are those of `{unrelated}`.")

    def test_the_lines_column_cannot_count_tests_or_be_blank_for_rust(self):
        for count in ["8 tests", "", "three"]:
            with self.subTest(count=count):
                self.fails("line count must be an integer", count=count)

    def test_the_count_is_newlines_including_blank_ones(self):
        self.fails("line count 2, but pin", count="2")
        self.write("src/lib.rs", SOURCE.rstrip("\n"))
        self.git("commit", "-qam", "unterminated last line")
        self.pin = self.git("rev-parse", "--short=12", "HEAD").strip()
        self.passes(body="`beta` (`src/lib.rs:3`).", count="2")

    def test_all_named_documents_are_checked(self):
        self.write("docs/a.md", self.document())
        result = self.run_tool(self.document(count="9"), "docs/a.md", "docs/b.md", name="docs/b.md")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("docs/b.md:11:", result.stdout)
        self.assertIn("2 files, 2 citations, 2 line-count rows checked", result.stdout)

    def test_a_missing_document_is_a_finding(self):
        result = self.run_tool(self.document(), "docs/missing.md")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("docs/missing.md:1: cannot read document", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
