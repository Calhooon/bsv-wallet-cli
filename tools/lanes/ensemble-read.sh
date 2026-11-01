#!/bin/zsh -f
# Trigger D's three read-only readers on a divergence or ruling call (docs/lanes/ENSEMBLE.md, #390):
#
#   tools/lanes/ensemble-read.sh <question-file> <evidence-A> <evidence-B> <evidence-C> [out-dir]
#
# Evidence A is the BRC text at its pin (stance reader-brc), B the reference implementation
# (reader-reference), C the package's code (reader-package). Each reader is its own
# `$CCA run --class review` holding only its stance, the question and its own evidence pointer,
# never another reader's output; the three run concurrently from this shell, which waits for all.
# Their outputs are reader-brc.txt, reader-reference.txt and reader-package.txt in the out
# directory (default a new directory under $TMPDIR); aggregate.py then prints the aggregation.
# Exit 0 on a consensus, 1 on a split, 2 on a usage error or a malformed answer.
#
# $CCA is the runner (default cca); a dry run stubs it with tools/lanes/stub-runner.sh.
# zsh -f: the caller's startup files are not read (the environment is inherited); a runner's
# stdin is /dev/null.
emulate -L zsh
setopt no_unset pipe_fail
zmodload zsh/datetime

CCA=${CCA:-cca}
# A reader runs on Opus only because the captain's standing word names Opus for cold reads.
REVIEW_MODEL=${REVIEW_MODEL:-opus}
HERE=${0:A:h}

if (( $# < 4 || $# > 5 )); then
  print -u2 "usage: ensemble-read.sh <question-file> <evidence-A> <evidence-B> <evidence-C> [out-dir]"
  exit 2
fi
question=$1
[[ -r $question ]] || { print -u2 "ensemble-read: cannot read $question"; exit 2; }
out=${5:-$(mktemp -d "${TMPDIR:-/Volumes/Lanes/targets}/ensemble-read.XXXXXX")}
mkdir -p -- $out

stamp() { strftime -s REPLY '%H:%M:%S' $EPOCHSECONDS; print -r -- "$REPLY.${${EPOCHREALTIME#*.}[1,3]}"; }

# read_one <stance> <evidence pointer>: one runner process, output in $out/<stance>.txt.
read_one() {
  local text="$(<$HERE/stances/$1.txt)

QUESTION:
$(<$question)

EVIDENCE: $2"
  print -r -- "ensemble-read: $1 launched $(stamp)"
  "$CCA" run --class review --model $REVIEW_MODEL "$text" < /dev/null > $out/$1.txt
  print -r -- "ensemble-read: $1 finished (runner exit $?) $(stamp)"
}

pids=()
read_one reader-brc "$2" &
pids+=($!)
read_one reader-reference "$3" &
pids+=($!)
read_one reader-package "$4" &
pids+=($!)
wait $pids

python3 -B $HERE/aggregate.py $out/reader-brc.txt $out/reader-reference.txt $out/reader-package.txt
code=$?
print -r -- "ensemble-read: outputs in $out"
exit $code
