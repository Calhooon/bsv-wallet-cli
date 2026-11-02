#!/usr/bin/env bash
# The chain scripts of tools/lanes (#390, #391), run against tools/lanes/stub-runner.sh standing
# in for cca and gh, with planted REPORTs and no delay:
#
#   zsh -f tools/test_chain_ensemble.sh   (CI's ratchet job runs it so; bash runs it too)
#
# chain-ensemble.sh: a lane whose whole output is the one line `REPORT: STATUS done` is judged
# done (#391's ask 2); a blocked lane stops with exit 3; two agreeing reviewers at a mean of 0.7
# or more run no third; a split runs C and says `split`; two agreeing at a mean below 0.7 run C
# and say `low mean` (ruling 3). ensemble-read.sh: readers answering `A ...`, `A, because` and
# `B` under `OPTIONS: A|B|C` aggregate as 2/3 on A; a reader answering `maybe` exits 2; a
# question without its OPTIONS line exits 2 (ruling 2); a bad OPTIONS line (a space inside a
# token, one token, a carriage return) exits 2 before any reader launches (#392); `Yes`, `yes`
# and `no` under `OPTIONS: yes|no` aggregate as 2/3 on yes (#392). The colon after a key is
# optional (#416): reviewers writing `VERDICT: merge` with `CONFIDENCE 0.9` and `VERDICT:merge`
# with `CONFIDENCE:0.8` aggregate as a merge, and a reader's `CONFIDENCE 0.8` reads. Each launch
# writes a fresh directory (#416): two chain launches and two read launches into one out
# directory each keep their own outputs. Each passing case prints its log's ENSEMBLE: and
# AGGREGATE: lines. Needs zsh, as the scripts do.
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
  if [ $ok = 1 ]; then
    echo "ok   $name"
    grep -E '^(ENSEMBLE|AGGREGATE): ' "$log" | sed 's/^/       /' || true
  else failures=$((failures + 1)); echo "FAIL $name"; sed 's/^/  | /' "$log"; fi
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

# The colon is optional (#416): the stub's reviewers write the variant forms.
mkdir -p "$WORK/colon/stub" "$WORK/colon/out"
printf 'REPORT: STATUS done\n' > "$WORK/colon/stub/lane.txt"
printf 'VERDICT: merge\nDECISION: planted\nCONFIDENCE 0.9\nRISKS: none\nRATIONALE: planted.\n' \
  > "$WORK/colon/stub/review-package.txt"
printf -- '- **VERDICT:**merge\nDECISION: planted\nCONFIDENCE:0.8\nRISKS none\nRATIONALE: planted.\n' \
  > "$WORK/colon/stub/review-wire-reference.txt"
code=0
STUB_DIR="$WORK/colon/stub" OUT_DIR="$WORK/colon/out" "$LANES/chain-ensemble.sh" lane-x lane/x \
  "$WORK/brief.txt" "$WORK/review.txt" "$WORK/review.txt" > "$WORK/colon/log" 2>&1 || code=$?
check "VERDICT: merge and VERDICT:merge with CONFIDENCE 0.9 and CONFIDENCE:0.8 merge" 0 $code \
  "$WORK/colon/log" "reviewer A: VERDICT merge CONFIDENCE 0.90" "reviewer B: VERDICT merge CONFIDENCE 0.80" \
  "ENSEMBLE: merge agreement=2/2 mean=0.85" "!review C"

# Two launches into one OUT_DIR (#416): launch 1 takes the empty directory, launch 2 a fresh
# subfolder of it; neither overwrites the other's lane.txt.
mkdir -p "$WORK/twice/out"
for n in 1 2; do
  mkdir -p "$WORK/twice/stub$n"
  printf 'REPORT: STATUS done\nlaunch %s\n' $n > "$WORK/twice/stub$n/lane.txt"
  form merge planted 0.9 > "$WORK/twice/stub$n/review-package.txt"
  form merge planted 0.8 > "$WORK/twice/stub$n/review-wire-reference.txt"
  code=0
  STUB_DIR="$WORK/twice/stub$n" OUT_DIR="$WORK/twice/out" "$LANES/chain-ensemble.sh" lane-x lane/x \
    "$WORK/brief.txt" "$WORK/review.txt" "$WORK/review.txt" > "$WORK/twice/log$n" 2>&1 || code=$?
  check "chain launch $n into one OUT_DIR" 0 $code "$WORK/twice/log$n" "ENSEMBLE: merge agreement=2/2 mean=0.85"
done
one=$(sed -n 's/^chain-ensemble: outputs in //p' "$WORK/twice/log1" | head -1)
second=$(sed -n 's/^chain-ensemble: outputs in //p' "$WORK/twice/log2" | head -1)
{ echo "one $one"; echo "second $second"; cat "$one/lane.txt" "$second/lane.txt"; } > "$WORK/twice/log"
[ "$one" = "$WORK/twice/out" ] && [ "${second%/*}" = "$WORK/twice/out" ] || code=4
check "two chain launches into one OUT_DIR keep both lane.txt" 0 $code "$WORK/twice/log" \
  "launch 1" "launch 2"

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

bad_options() { # bad_options <case> <question first line> <expected reason>
  local dir="$WORK/$1"
  mkdir -p "$dir/out"
  printf '%s\nDoes the row stand?\n' "$2" > "$dir/question.txt"
  code=0
  STUB_DIR="$WORK/options/stub" "$LANES/ensemble-read.sh" "$dir/question.txt" brc@pin ts@pin cli@rev \
    "$dir/out" > "$dir/log" 2>&1 || code=$?
  check "a bad OPTIONS line exits 2 before any reader: $1" 2 $code "$dir/log" \
    "the OPTIONS line of $dir/question.txt $3" "!launched"
}
bad_options space-in-token 'OPTIONS: A B|C' "has a space inside the token 'A B'"
bad_options one-token 'OPTIONS: A' "must name two to four tokens joined by |"
bad_options carriage-return $'OPTIONS: A|B|C\r' "carries a carriage return"

read_case case 'OPTIONS: yes|no' 'Yes, the row stands' 'yes' 'no'
check "readers Yes, yes, no under yes|no aggregate as 2/3 on yes" 0 $code "$WORK/case/log" \
  "AGGREGATE: consensus agreement=2/3 mean=0.77 decision: yes"

# A reader's CONFIDENCE without its colon reads (#416).
read_case colon-read 'OPTIONS: A|B|C' 'A the row stands' 'A' 'B'
sed -i.bak 's/^CONFIDENCE: /CONFIDENCE /' "$WORK/colon-read/stub/reader-brc.txt"
code=0
STUB_DIR="$WORK/colon-read/stub" "$LANES/ensemble-read.sh" "$WORK/colon-read/question.txt" brc@pin ts@pin \
  cli@rev "$WORK/colon-read/out" > "$WORK/colon-read/log" 2>&1 || code=$?
check "a reader's CONFIDENCE 0.8 without its colon reads" 0 $code "$WORK/colon-read/log" \
  "CONFIDENCE 0.80 DECISION: A the row stands" "AGGREGATE: consensus agreement=2/3 mean=0.77 decision: A"

# Two read launches into one out directory (#416): the second gets a fresh subfolder.
code=0
STUB_DIR="$WORK/options/stub" "$LANES/ensemble-read.sh" "$WORK/options/question.txt" brc@pin ts@pin cli@rev \
  "$WORK/options/out" > "$WORK/options/log2" 2>&1 || code=$?
again=$(sed -n 's/^ensemble-read: outputs in //p' "$WORK/options/log2" | head -1)
{ echo "again $again"; ls "$WORK/options/out" "$again"; } >> "$WORK/options/log2"
[ "${again%/*}" = "$WORK/options/out" ] && [ -f "$WORK/options/out/reader-brc.txt" ] \
  && [ -f "$again/reader-brc.txt" ] || code=4
check "a second read launch into one out directory takes a fresh subfolder" 0 $code "$WORK/options/log2" \
  "AGGREGATE: consensus agreement=2/3 mean=0.77 decision: A"

echo "$cases cases, $failures failed"
[ $failures = 0 ]
