#!/usr/bin/env bash
# The chain scripts of tools/lanes (#390, #391), run against tools/lanes/stub-runner.sh standing
# in for cca and gh, with planted REPORTs and no delay:
#
#   bash tools/test_chain_ensemble.sh
#
# chain-ensemble.sh: a lane whose whole output is the one line `REPORT: STATUS done` is judged
# done (#391's ask 2); a blocked lane stops with exit 3; two agreeing reviewers at a mean of 0.7
# or more run no third; a split runs C and says `split`; two agreeing at a mean below 0.7 run C
# and say `low mean` (ruling 3). ensemble-read.sh: readers answering `A ...`, `A, because` and
# `B` under `OPTIONS: A|B|C` aggregate as 2/3 on A; a reader answering `maybe` exits 2; a
# question without its OPTIONS line exits 2 (ruling 2). Needs zsh, as the scripts do.
# CHAIN_ENSEMBLE_LANES points the cases at another copy of tools/lanes.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
LANES="${CHAIN_ENSEMBLE_LANES:-$ROOT/tools/lanes}"
WORK=$(mktemp -d "${TMPDIR:-${RUNNER_TEMP:-/tmp}}/chain-ensemble-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
export CCA="$LANES/stub-runner.sh" GH="$LANES/stub-runner.sh" STUB_DELAY=0
unset PR_NUMBER STUB_REPORT STUB_BRIEFS
printf 'Review PR @PR@ in full.\n' > "$WORK/review.txt"
printf 'Do the lane.\n' > "$WORK/brief.txt"

failures=0 cases=0
check() { # check <name> <expected exit> <got exit> <output file> <expected line>... (! line: absent)
  local name=$1 want=$2 got=$3 log=$4 ok=1; shift 4
  cases=$((cases + 1))
  [ "$got" = "$want" ] || { ok=0; echo "  exit $got, expected $want"; }
  for line in "$@"; do
    if [ "${line#!}" != "$line" ]; then
      ! grep -qF -- "${line#!}" "$log" || { ok=0; echo "  present: ${line#!}"; }
    else
      grep -qF -- "$line" "$log" || { ok=0; echo "  missing: $line"; }
    fi
  done
  if [ $ok = 1 ]; then echo "ok   $name"; else failures=$((failures + 1)); echo "FAIL $name"; sed 's/^/  | /' "$log"; fi
}

form() { # form <verdict or -> <decision> <confidence>
  [ "$1" = - ] || printf 'VERDICT %s\n' "$1"
  printf 'DECISION: %s\nCONFIDENCE: %s\nRISKS: none\nRATIONALE: planted.\n' "$2" "$3"
}

chain() { # chain <case> <lane report> <A verdict> <A conf> <B verdict> <B conf> [<C verdict> <C conf>]
  local dir="$WORK/$1"
  mkdir -p "$dir/stub" "$dir/out"
  printf '%s\n' "$2" > "$dir/stub/lane.txt"
  form "$3" "planted" "$4" > "$dir/stub/review-package.txt"
  form "$5" "planted" "$6" > "$dir/stub/review-wire-reference.txt"
  [ $# -lt 8 ] || form "$7" "planted" "$8" > "$dir/stub/review-adversarial.txt"
  code=0
  STUB_DIR="$dir/stub" OUT_DIR="$dir/out" "$LANES/chain-ensemble.sh" lane-x lane/x \
    "$WORK/brief.txt" "$WORK/review.txt" "$WORK/review.txt" > "$dir/log" 2>&1 || code=$?
}

chain one-line 'REPORT: STATUS done' merge 0.9 merge 0.8
check "a one-line REPORT: STATUS done is judged done" 0 $code "$WORK/one-line/log" \
  "ENSEMBLE: merge agreement=2/2 mean=0.85" "!not STATUS done"

chain blocked $'REPORT: STATUS blocked\nthe gate is red' merge 0.9 merge 0.8
check "a blocked lane stops before the reviews" 3 $code "$WORK/blocked/log" \
  "the lane's REPORT is not STATUS done: REPORT: STATUS blocked" "!review A"

chain agree $'REPORT: STATUS done\nbase abc' merge 0.9 merge 0.8
check "two agreeing at mean 0.85 run no third" 0 $code "$WORK/agree/log" \
  "ENSEMBLE: merge agreement=2/2 mean=0.85" "!review C"

chain split $'REPORT: STATUS done\nbase abc' merge 0.9 fix 0.7 merge 0.7
check "a split runs C and says split" 0 $code "$WORK/split/log" \
  "A and B split (agreement=1/2 mean=0.80): review C launched" "ENSEMBLE: merge agreement=2/3 mean=0.77"

chain low-mean $'REPORT: STATUS done\nbase abc' merge 0.6 merge 0.7 merge 0.8
check "two agreeing at mean 0.65 run C and say low mean" 0 $code "$WORK/low-mean/log" \
  "A and B low mean (agreement=2/2 mean=0.65): review C launched" "ENSEMBLE: merge agreement=3/3 mean=0.70"

read_case() { # read_case <case> <question first line> <brc decision> <reference decision> <package decision>
  local dir="$WORK/$1"
  mkdir -p "$dir/stub" "$dir/out"
  printf '%s\nDoes the row stand?\n' "$2" > "$dir/question.txt"
  form - "$3" 0.8 > "$dir/stub/reader-brc.txt"
  form - "$4" 0.8 > "$dir/stub/reader-reference.txt"
  form - "$5" 0.7 > "$dir/stub/reader-package.txt"
  code=0
  STUB_DIR="$dir/stub" "$LANES/ensemble-read.sh" "$dir/question.txt" brc@pin ts@pin cli@rev "$dir/out" \
    > "$dir/log" 2>&1 || code=$?
}

read_case options 'OPTIONS: A|B|C' 'A the row stands' 'A, because the reference agrees' 'B'
check "readers A ..., A, because, B aggregate as 2/3 on A" 0 $code "$WORK/options/log" \
  "AGGREGATE: consensus agreement=2/3 mean=0.77 decision: A"

read_case maybe 'OPTIONS: A|B|C' 'A the row stands' 'maybe' 'B'
check "a reader answering maybe exits 2" 2 $code "$WORK/maybe/log" \
  "reader-reference.txt: DECISION must begin with exactly one of A|B|C, got maybe"

read_case no-options 'Does the row stand?' 'A' 'A' 'A'
check "a question without its OPTIONS line exits 2" 2 $code "$WORK/no-options/log" \
  "the first line of $WORK/no-options/question.txt must be OPTIONS:" "!launched"

echo "$cases cases, $failures failed"
[ $failures = 0 ]
