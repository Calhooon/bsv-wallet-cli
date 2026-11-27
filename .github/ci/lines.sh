#!/usr/bin/env bash
# The direction rule's two checks (RELEASING.md, "The direction rule"): every change is born on
# beta, and nothing sits on main that beta lacks. Run by .github/workflows/lines.yml, which
# fetches the other line before it calls this file; this file reads local refs alone.
#
#   lines.sh main-in-beta   every commit of MAIN_REF after MAIN_BASELINE is on HEAD_REF
#   lines.sh beta-born      every commit of the pull request (PR_BASE..PR_HEAD) is on BETA_REF
#
# A commit is on a line by sha, by patch (git cherry), or by a "(cherry picked from commit
# <sha>)" line in either direction: the commit names a source the line holds, or a commit of
# the line names it. Two shapes are main-born by nature and are let through, each said in the
# log: a hop commit (the stable pins and the lock: Cargo.toml lines of the bsv-* crates and
# Cargo.lock, nothing else), in both checks; a version commit (Cargo.toml's and Cargo.lock's
# version, CHANGELOG.md's added section, the assembled changelog.d fragments deleted), in
# beta-born alone: beta carries its link.
set -euo pipefail

MAIN_REF="${MAIN_REF:-origin/main}"
BETA_REF="${BETA_REF:-origin/beta}"

# the -/+ lines a commit gives one path
changed() {
  git diff -U0 "$1^" "$1" -- "$2" | grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' || true
}

# does the commit name, by a cherry-pick line, a source that $2 holds
source_on() {
  local src
  while read -r src; do
    [ -n "$src" ] || continue
    if git cat-file -e "${src}^{commit}" 2>/dev/null && git merge-base --is-ancestor "$src" "$2"; then
      return 0
    fi
  done <<< "$(git log -1 --format=%B "$1" | sed -n 's/.*(cherry picked from commit \([0-9a-f]\{40\}\)).*/\1/p')"
  return 1
}

# a hop commit: every changed path is a Cargo.lock or a Cargo.toml, each modified, and every
# changed Cargo.toml line is the requirement of a bsv-* crate
hop_commit() {
  local status path lines seen=0
  while IFS=$'\t' read -r status path; do
    [ -n "$status" ] || continue
    [ "$status" = M ] || return 1
    case "${path##*/}" in
      Cargo.lock) ;;
      Cargo.toml)
        lines="$(changed "$1" "$path")"
        [ -n "$lines" ] || return 1
        if grep -qvE '^[-+]bsv-[A-Za-z0-9_-]+ = ' <<< "$lines"; then return 1; fi
        ;;
      *) return 1 ;;
    esac
    seen=1
  done <<< "$(git diff-tree --no-commit-id --name-status -r "$1")"
  [ "$seen" = 1 ]
}

# a version commit: Cargo.toml's version (and Cargo.lock's), CHANGELOG.md gaining lines and
# losing none but a heading or a blank, changelog.d fragments deleted; nothing else
version_commit() {
  local status path lines seen=0
  while IFS=$'\t' read -r status path; do
    [ -n "$status" ] || continue
    case "$path" in
      Cargo.toml | Cargo.lock | CHANGELOG.md) [ "$status" = M ] || return 1 ;;
      changelog.d/README.md) return 1 ;;
      changelog.d/*.md) [ "$status" = D ] || return 1 ;;
      *) return 1 ;;
    esac
    seen=1
  done <<< "$(git diff-tree --no-commit-id --name-status -r "$1")"
  [ "$seen" = 1 ] || return 1
  lines="$(changed "$1" Cargo.toml)"
  grep -qE '^\+version = "[^"]*"$' <<< "$lines" || return 1
  if grep -qvE '^[-+]version = "[^"]*"$' <<< "$lines"; then return 1; fi
  lines="$(changed "$1" Cargo.lock)"
  if [ -n "$lines" ] && grep -qvE '^[-+]version = "[^"]*"$' <<< "$lines"; then return 1; fi
  lines="$(changed "$1" CHANGELOG.md | grep -E '^-' || true)"
  if [ -n "$lines" ] && grep -qvE '^-(## \[[^]]*\]( - [0-9]{4}-[0-9]{2}-[0-9]{2})?)?$' <<< "$lines"; then return 1; fi
  return 0
}

# $1: the line or tree the commits must be on; $2: the log that may name them; $3: git cherry's
# list; $4: "version" when a version commit is let through
judge() {
  local mark sha missing=() read_count=0
  while read -r mark sha _; do
    [ "$mark" = "+" ] || continue                 # "-": a patch-equivalent commit is on the line
    read_count=$((read_count + 1))
    if source_on "$sha" "$1"; then continue; fi   # the commit names a source the line holds
    if grep -qF "(cherry picked from commit ${sha})" <<< "$2"; then continue; fi   # the line names it
    if hop_commit "$sha"; then
      echo "a hop commit, main-born by nature (the bsv-* pins and the lock alone): $(git log -1 --format='%h %s' "$sha")"
      continue
    fi
    if [ "$4" = version ] && version_commit "$sha"; then
      echo "a version commit, main-born by nature (the version, CHANGELOG.md's section, the assembled fragments): $(git log -1 --format='%h %s' "$sha")"
      continue
    fi
    missing+=("$sha")
  done <<< "$3"
  if [ "${#missing[@]}" -eq 0 ]; then return 0; fi
  for sha in "${missing[@]}"; do git log -1 --format='  %h %s' "$sha"; done
  return 1
}

main_in_beta() {
  : "${MAIN_BASELINE:?MAIN_BASELINE is not set: the commit of main the rule is read from}"
  local head="${HEAD_REF:-HEAD}" base linked list
  if ! git merge-base --is-ancestor "$MAIN_BASELINE" "$MAIN_REF"; then
    echo "::error::the baseline ${MAIN_BASELINE} is not on ${MAIN_REF}"
    exit 1
  fi
  base="$(git merge-base "$MAIN_REF" "$head")"
  linked="$(git log --format=%B "$base..$head")"
  list="$(git cherry "$head" "$MAIN_REF" "$MAIN_BASELINE")"
  echo "main-in-beta: $(git rev-parse --short "$MAIN_BASELINE")..$(git rev-parse --short "$MAIN_REF") against $(git rev-parse --short "$head")"
  if judge "$head" "$linked" "$list" none; then
    echo "every commit of main since the baseline is on this tree by sha, patch or cherry-pick link, or is a hop commit"
    exit 0
  fi
  echo "::error::main has the commits above that beta lacks (no patch-equivalent, no cherry-pick link, not a hop commit); a version commit is linked by a commit on beta that adds its released section to CHANGELOG.md and carries its (cherry picked from commit <sha>) line"
  exit 1
}

beta_born() {
  : "${PR_BASE:?PR_BASE is not set}" "${PR_HEAD:?PR_HEAD is not set}"
  local base linked list merges
  base="$(git merge-base "$PR_BASE" "$PR_HEAD")"
  linked="$(git log --format=%B "$(git merge-base "$BETA_REF" "$PR_HEAD")..$BETA_REF")"
  list="$(git cherry "$BETA_REF" "$PR_HEAD" "$base")"
  merges="$(git rev-list --merges --count "$base..$PR_HEAD")"
  echo "beta-born: $(git rev-parse --short "$base")..$(git rev-parse --short "$PR_HEAD") against beta $(git rev-parse --short "$BETA_REF")"
  if [ "$merges" != 0 ]; then
    echo "::notice::${merges} merge commit(s) in the pull request are not read (git cherry omits a merge); the cut is the one pull request that carries one"
  fi
  if judge "$BETA_REF" "$linked" "$list" version; then
    echo "every commit of the pull request is on beta by sha, patch or cherry-pick link, or is a hop commit or a version commit"
    exit 0
  fi
  echo "::error::the commits above were not born on beta; land each on beta through a pull request, then cherry-pick it here with -x"
  exit 1
}

case "${1:-}" in
  main-in-beta) main_in_beta ;;
  beta-born) beta_born ;;
  *) echo "usage: lines.sh main-in-beta|beta-born" >&2; exit 2 ;;
esac
