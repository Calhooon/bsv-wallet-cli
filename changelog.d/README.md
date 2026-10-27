# Changelog fragments

Add one file per change, named `<number>.md` or `<number>-<slug>.md`. The number
is the issue number, or the PR number when there is no issue; a slug is words
or numbers separated by hyphens. This directory holds only this README and
fragment files.

Write the entry in the same words as a CHANGELOG bullet today: `- **...**` or
a paragraph, over any number of lines. A fragment must be non-empty UTF-8 and
carry no fenced or indented code blocks; use inline backticks only. For every
line, strip leading whitespace and repeated container prefixes (`> `, `- `,
`* `, `+ `, or 1 to 9 digits followed by `. ` or `) `). The remainder must not
begin with `#` or consist only of `=` or only of `-`, with or without spaces
between them. Indentation, fences and preceding blank lines never exempt a
line; write a horizontal rule as `***`. Everything else is allowed. Entries
are assembled by ascending number, then filename, with one blank line between
entries.

A PR changing `src/`, `mcp/src/`, `tests/e2e/` or `Cargo.toml` adds a fragment
or carries `no-changelog`. That label means tests only, docs only, a re-record,
or CI wiring. Editing, deleting or renaming an unchanged fragment does not
count as adding one. Never add a line to CHANGELOG.md's Unreleased section.
Adding a label does not trigger CI (`pull_request` runs on opened, synchronize
and reopened); a `no-changelog` label added after a failure counts when the
ratchet job is re-run.

From the repository root:

```sh
python3 -B tools/changelog --list
python3 -B tools/changelog --check --base origin/beta
python3 -B tools/changelog --check --base origin/beta --allow-missing
python3 -B tools/changelog --assemble 0.19.0-beta.1
```

`--allow-missing` is the CI label exception; malformed fragments still fail.
`--list` and `--check` never write. Assembly moves the current Unreleased body
verbatim into the new version section ahead of the fragments, leaves Unreleased
empty, and deletes the assembled files while keeping this README. Its date
defaults to today in UTC; `--date YYYY-MM-DD` fixes it explicitly. Assembly
refuses an invalid semantic version or a version already present. With no
pending entries it also refuses unless `--allow-empty` is used; that flag writes
`No changes since ...` using the release heading directly below the new section,
read from CHANGELOG.md.

If assembly is interrupted, check whether CHANGELOG.md contains the new version's heading. If it does, assembly is complete: remove any leftover `.CHANGELOG.md.*` file and restore nothing. If it does not, run `git restore changelog.d`, remove any leftover `.CHANGELOG.md.*` file, then run assembly again.

Commit the result with the release's version bump as [RELEASING.md](../RELEASING.md)
describes.
