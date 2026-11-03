#!/usr/bin/env bash
# The changed paths, read once (#476), moved from the `changes` job's step
# "The changed paths" in .github/workflows/ci.yml (#509) so the rule is one
# file the workflow and tools/test_docs_only.py both run.
#
#   docs_only.sh            the event's diff (the step: EVENT, REF, SHA,
#                           BEFORE, GROUP_BASE, GROUP_HEAD from the event)
#   docs_only.sh --paths    the changed paths on stdin, one a line, no git
#                           and no test selection (the test's mode)
#
# Both read DOCS_READ_BY_CODE and PROSE_IN_CODE_FOLDERS (ci.yml's env) and
# write docs_only=<true|false> to $GITHUB_OUTPUT; $RUNNER_TEMP holds the
# non-docs paths for .github/ci/affected.py. docs_only is true when every
# changed path is docs: every path ending `.md` but DOCS_READ_BY_CODE and
# every path under tests/ or fuzz/ (a code-scanned folder is never docs) but
# PROSE_IN_CODE_FOLDERS. The answer never fails the job.
# -e is the step's default shell's (bash -eo pipefail), kept by the move.
set -euo pipefail
answer() {
  echo "docs_only=$1" >> "$GITHUB_OUTPUT"
  echo "changes: docs_only=$1 ($2)"
  exit 0
}
mode="diff"
if [ "${1:-}" = --paths ]; then
  mode="paths"
  paths=$(cat)
  [ -n "$paths" ] || answer false "no changed path"
  echo "changes: --paths, the paths of stdin"
else
  case "$EVENT" in
    pull_request) base=HEAD^1 head=HEAD ;;
    merge_group) base=$GROUP_BASE head=$GROUP_HEAD ;;
    push)
      [[ $REF == refs/heads/* ]] || answer false "a push of $REF"
      base=$BEFORE head=$SHA ;;
    *) answer false "$EVENT: the full set" ;;
  esac
  [ -n "$base" ] && [ -n "${base//0/}" ] && [ -n "$head" ] || answer false "no base for $EVENT"
  for rev in "$base" "$head"; do
    git cat-file -e "$rev^{commit}" 2> /dev/null \
      || git fetch --no-tags --filter=blob:none origin "$rev" \
      || answer false "$rev is not fetchable"
  done
  paths=$(git diff --no-renames --name-only "$base" "$head") || answer false "git diff $base $head failed"
  [ -n "$paths" ] || answer false "no changed path"
  echo "changes: $EVENT, $base..$head"
fi
docs=true
code="$RUNNER_TEMP/code-paths"
: > "$code"
while IFS= read -r path; do
  kind=docs
  case "$path" in *.md) ;; *) kind="not docs" ;; esac
  case "$path" in
    tests/* | fuzz/*)
      if [ "$kind" = docs ]; then kind="not docs (a code-scanned folder)"; fi
      for prose in $PROSE_IN_CODE_FOLDERS; do
        if [ "$path" = "$prose" ]; then kind=docs; fi
      done ;;
  esac
  for read_by_code in $DOCS_READ_BY_CODE; do
    if [ "$path" = "$read_by_code" ]; then kind="not docs (the code reads it)"; fi
  done
  if [ "$kind" != docs ]; then docs=false; echo "$path" >> "$code"; fi
  echo "  $kind: $path"
done <<< "$paths"
# The tests the diff reaches (#473): on a PR the targets of its
# non-docs paths, else the whole suite. A failed selector writes
# nothing, and nothing is the whole suite.
if [ "$docs" != true ] && [ "$mode" = diff ]; then
  python3 -B .github/ci/affected.py --event "$EVENT" --paths "$code" --github-output "$GITHUB_OUTPUT" \
    || echo "changes: the selector failed (#473); every job runs the whole suite"
fi
answer "$docs" "$(wc -l <<< "$paths" | tr -d ' ') changed paths"
