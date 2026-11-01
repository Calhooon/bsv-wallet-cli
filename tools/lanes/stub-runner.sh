#!/bin/zsh -f
# A dry-run stand-in for cca and gh (#390), so the chain scripts run without seats:
#   CCA=$PWD/tools/lanes/stub-runner.sh GH=$PWD/tools/lanes/stub-runner.sh tools/lanes/chain-ensemble.sh ...
#
# As cca (`stub-runner.sh run --class C [--model M] [--worktree W] "<brief>"`) it prints a REPORT:
# the contents of $STUB_REPORT when set; else $STUB_DIR/<stance>.txt, the stance read from the
# brief's `STANCE: <name>` line; else $STUB_DIR/lane.txt for a brief without one. It sleeps
# $STUB_DELAY seconds (default 2) between its start and end lines on stderr, each stamped with
# the wall clock, so concurrent calls show overlapping intervals.
# With $STUB_BRIEFS set, the brief it was given is saved there as <stance or lane>.txt.
# As gh (`stub-runner.sh pr list ...`) it prints $STUB_PR (default 999).
emulate -L zsh
setopt err_exit no_unset pipe_fail
zmodload zsh/datetime

if [[ ${1:-} == pr ]]; then
  print -r -- "${STUB_PR:-999}"
  exit 0
fi
[[ ${1:-} == run ]] || { print -u2 "stub-runner: expected run or pr, got: $*"; exit 2; }
shift
class=?
while (( $# > 1 )); do
  case $1 in
    --class) class=$2; shift 2 ;;
    --model|--worktree|--host) shift 2 ;;
    --*) shift ;;
    *) break ;;
  esac
done
brief=${1:-}
stance=${${(M)${(f)brief}:#STANCE: *}[1]:-}
stance=${${stance#STANCE: }%% *}
label=${stance:-lane}

stamp() { strftime -s REPLY '%H:%M:%S' $EPOCHSECONDS; print -r -- "$REPLY.${${EPOCHREALTIME#*.}[1,3]}"; }
[[ -n ${STUB_BRIEFS:-} ]] && print -r -- "$brief" > $STUB_BRIEFS/$label.txt
print -u2 "stub-runner: start $label (class $class) $(stamp)"
sleep ${STUB_DELAY:-2}
if [[ -n ${STUB_REPORT:-} ]]; then
  print -r -- "$STUB_REPORT"
elif [[ -n ${STUB_DIR:-} && -f $STUB_DIR/$label.txt ]]; then
  cat -- "$STUB_DIR/$label.txt"
else
  print -u2 "stub-runner: no STUB_REPORT and no ${STUB_DIR:-\$STUB_DIR}/$label.txt"
  exit 2
fi
print -u2 "stub-runner: end $label $(stamp)"
