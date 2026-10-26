# Changelog fragments

Add one file per change, named `<number>.md` or `<number>-<slug>.md`. The number
is the issue number, or the PR number when there is no issue; a slug is words
or numbers separated by hyphens. This directory holds only this README and
fragment files.

Write the entry in the same words as a CHANGELOG bullet today: `- **...**` or
a paragraph, over any number of lines. A fragment must be non-empty UTF-8 and
have no Markdown heading. Entries are assembled by ascending number, then
filename, with one blank line between entries.

A PR changing `src/`, `mcp/src/`, `tests/e2e/` or `Cargo.toml` adds a fragment
or carries `no-changelog`. That label means tests only, docs only, a re-record,
or CI wiring. Editing or deleting an existing fragment does not count as
adding one. Never add a line to CHANGELOG.md's Unreleased section.

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
refuses an invalid semantic version, a version already present, or no entries.
Commit the result with the release's version bump as [RELEASING.md](../RELEASING.md)
describes.
