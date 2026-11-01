#!/bin/zsh -f
# The lane, then trigger B's cold review (docs/lanes/ENSEMBLE.md, #390):
#
#   tools/lanes/chain-ensemble.sh <lane-name> <branch> <brief> <review-A> <review-B> [review-C]
#
# Runs the lane (`$CCA run --class code --worktree <lane-name> "<brief text>"`) and stops unless
# its REPORT says `STATUS done`. Finds the PR by its head branch with gh, fills `@PR@` in each
# review brief, prepends the stance (A: review-package, B: review-wire-reference) and runs A and B
# concurrently from this shell: two runner processes, each its own session, so the lane rule's
# foreground clause holds (this shell is the caller and waits for both). Aggregates their bounded
# forms with aggregate.py; only on a split runs C (default: B's brief) under review-adversarial,
# with A's and B's outputs appended under a heading it reads after forming its own verdict.
# Prints each reviewer's verdict line and one line
#   ENSEMBLE: merge|fix|split agreement=<k>/<n> mean=<0.xx>
# Exit 0 on a merge consensus, 3 otherwise (a fix consensus, a split, a lane not done, a
# malformed review). Never merges anything.
#
# $CCA is the runner (default cca) and $GH the GitHub client (default gh); a dry run stubs both
# with tools/lanes/stub-runner.sh. zsh -f: the caller's startup files are not read (the
# environment is inherited); a runner's stdin is /dev/null. $OUT_DIR keeps the outputs
# (default a new directory under $TMPDIR). $PR_NUMBER skips the lookup.
emulate -L zsh
setopt no_unset pipe_fail
zmodload zsh/datetime

CCA=${CCA:-cca}
GH=${GH:-gh}
# A review runs on Opus only because the captain's standing word names Opus for cold reads.
REVIEW_MODEL=${REVIEW_MODEL:-opus}
HERE=${0:A:h}
STANCES=$HERE/stances

if (( $# < 5 || $# > 6 )); then
  print -u2 "usage: chain-ensemble.sh <lane-name> <branch> <brief> <review-A> <review-B> [review-C]"
  exit 2
fi
lane=$1 branch=$2 brief=$3 rev_a=$4 rev_b=$5 rev_c=${6:-$5}
for f in $brief $rev_a $rev_b $rev_c; do
  [[ -r $f ]] || { print -u2 "chain-ensemble: cannot read $f"; exit 2; }
done
out=${OUT_DIR:-$(mktemp -d "${TMPDIR:-/Volumes/Lanes/targets}/chain-ensemble.XXXXXX")}
mkdir -p -- $out

stamp() { strftime -s REPLY '%H:%M:%S' $EPOCHSECONDS; print -r -- "$REPLY.${${EPOCHREALTIME#*.}[1,3]}"; }
say() { print -r -- "chain-ensemble: $* $(stamp)"; }

# The lane.
say "lane $lane launched"
"$CCA" run --class code --worktree $lane "$(cat -- $brief)" < /dev/null > $out/lane.txt
say "lane $lane finished (runner exit $?)"
status_line=${${(M)${(f)"$(<$out/lane.txt)"}:#*REPORT: STATUS *}[1]:-}
if [[ $status_line != *"REPORT: STATUS done"* ]]; then
  print -r -- "chain-ensemble: the lane's REPORT is not STATUS done: ${status_line:-no REPORT line} ($out/lane.txt)"
  exit 3
fi

# The PR.
pr=${PR_NUMBER:-$("$GH" pr list --head $branch --state open --json number --jq '.[0].number')}
if [[ -z $pr || $pr == null ]]; then
  print -r -- "chain-ensemble: no open PR with head $branch"
  exit 3
fi
say "PR $pr found for $branch"

# review <label> <stance> <brief> [appendix]: one runner process, output in $out/review-<label>.txt.
review() {
  local text="$(<$STANCES/$2.txt)

${"$(<$3)"//@PR@/$pr}"
  [[ -n ${4:-} ]] && text+="

$4"
  say "review $1 ($2) launched"
  "$CCA" run --class review --model $REVIEW_MODEL "$text" < /dev/null > $out/review-$1.txt
  say "review $1 ($2) finished (runner exit $?)"
}

review A review-package $rev_a &
pid_a=$!
review B review-wire-reference $rev_b &
pid_b=$!
wait $pid_a $pid_b

# tally <files...>: aggregate the bounded forms; sets $agg_code, $agg, $fraction, $mean, $modal.
tally() {
  agg=$(python3 -B $HERE/aggregate.py "$@")
  agg_code=$?
  if (( agg_code == 2 )); then
    print -r -- "ENSEMBLE: malformed (aggregate's message above; outputs in $out)"
    exit 3
  fi
  local summary=${${(M)${(f)agg}:#AGGREGATE: *}[1]}
  local words=(${=summary})
  fraction=${words[3]} mean=${words[4]} modal=${summary#*decision: }
}

tally $out/review-A.txt $out/review-B.txt
if (( agg_code == 1 )); then
  say "A and B split ($fraction $mean): review C launched"
  appendix="---- THE TWO REVIEWERS' VERDICTS: read only after your own verdict is written ----
REVIEWER A (review-package):
$(<$out/review-A.txt)

REVIEWER B (review-wire-reference):
$(<$out/review-B.txt)"
  review C review-adversarial $rev_c "$appendix"
  tally $out/review-A.txt $out/review-B.txt $out/review-C.txt
fi
for line in ${(f)agg}; do
  [[ $line == $out/review-* ]] && print -r -- "reviewer ${${line#$out/review-}%%.txt:*}: ${line#*.txt: }"
done
if (( agg_code == 0 )); then
  print -r -- "ENSEMBLE: $modal $fraction $mean"
else
  print -r -- "ENSEMBLE: split $fraction $mean"
fi
print -r -- "chain-ensemble: outputs in $out"
(( agg_code == 0 )) && [[ $modal == merge ]] && exit 0
exit 3
