#!/usr/bin/env bash
# The differential job's step "Run the differential" (#70), run outside
# GitHub: its script body is read from .github/workflows/ci.yml as it stands
# and run under the runner's shells (`bash -e`, GitHub's default for a run
# step, and `bash -e -o pipefail`, the explicit `shell: bash`) against a
# stand-in `gh` (the label read) and a stand-in `python3` (tools/differential,
# which writes a report line and exits with the asked code).
#
#   bash tools/test_differential_step.sh
#
# The eight cases are the tool's four exit codes under the two label states
# with REFACTOR_FAILS false (as the job holds it until toolbox #105); then the
# refactor failure switched on, and a failed label read. Each case prints its
# step's exit and the last line it wrote.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CI="${DIFFERENTIAL_STEP_CI:-$ROOT/.github/workflows/ci.yml}"
WORK=$(mktemp -d "${TMPDIR:-${RUNNER_TEMP:-/tmp}}/differential-step.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# The body: the lines under the step's `run: |`, to the first line indented
# less than the block, with the block's indent removed.
awk '
  /^      - name: Run the differential$/ { step = 1; next }
  step && /^      - / { exit }
  step && /^        run: \|$/ { body = 1; next }
  body {
    if ($0 != "" && $0 !~ /^          /) exit
    sub(/^          /, ""); print
  }
' "$CI" > "$WORK/body.sh"
grep -q 'tools/differential' "$WORK/body.sh" || { echo "FAIL: the step's body was not found in $CI"; exit 1; }

mkdir -p "$WORK/bin"
cat > "$WORK/bin/gh" <<'EOF'
#!/usr/bin/env bash
[ "$FAKE_GH" = fail ] && { echo "gh: HTTP 502" >&2; exit 1; }
echo "$FAKE_GH"
EOF
cat > "$WORK/bin/python3" <<'EOF'
#!/usr/bin/env bash
[ "$1" = tools/differential ] || { echo "stand-in python3: unexpected $*" >&2; exit 99; }
while [ $# -gt 0 ]; do [ "$1" = --out ] && out=$2; shift; done
case "$FAKE_CODE" in
  0) echo "DIFFERENTIAL: no differences over 335 calls and 35 commands" > "$out/report.txt" ;;
  1) echo "DIFFERENTIAL: 13 differences over 335 calls and 35 commands" > "$out/report.txt" ;;
  3) echo "DIFFERENTIAL: incomplete recording" > "$out/report.txt" ;;
  *) : ;; # a suite run failed: no report
esac
exit "$FAKE_CODE"
EOF
chmod +x "$WORK/bin/gh" "$WORK/bin/python3"

fails=0
# case NAME FAKE_GH FAKE_CODE REFACTOR_FAILS WANT_EXIT WANT_TAIL_PREFIX
case_() {
  local name=$1 gh=$2 code=$3 fails_on=$4 want_exit=$5 want_tail=$6 shell opts
  for shell in "bash -e" "bash -e -o pipefail"; do
    read -ra opts <<< "${shell#bash }"
    local run="$WORK/run" got_exit tail
    rm -rf "$run"; mkdir -p "$run"
    got_exit=0
    (cd "$ROOT" && env PATH="$WORK/bin:$PATH" FAKE_GH="$gh" FAKE_CODE="$code" \
      REFACTOR_FAILS="$fails_on" GH_TOKEN=x PR_NUMBER=1 GITHUB_REPOSITORY=o/r \
      RUNNER_TEMP="$run" GITHUB_STEP_SUMMARY="$run/summary" \
      bash --noprofile --norc "${opts[@]}" "$WORK/body.sh") > "$run/out" 2>&1 || got_exit=$?
    tail=$(tail -n 1 "$run/out")
    if [ "$got_exit" = "$want_exit" ] && [[ "$tail" == "$want_tail"* ]]; then
      echo "ok   $name [$shell]: exit $got_exit, tail: $tail"
    else
      echo "FAIL $name [$shell]: exit $got_exit (want $want_exit), tail: $tail (want $want_tail...)"
      fails=$((fails + 1))
    fi
  done
}

case_ "1 code 0, no label"           false 0 false 0 "::notice::DIFFERENTIAL: no differences"
case_ "2 code 0, label"              true  0 false 0 "::notice::DIFFERENTIAL: no differences"
case_ "3 code 1, no label"           false 1 false 0 "::notice::DIFFERENTIAL: 13 differences over 335 calls and 35 commands (no refactor label"
case_ "4 code 1, label (held)"       true  1 false 0 "::warning::DIFFERENTIAL: 13 differences over 335 calls and 35 commands on a PR labelled refactor: reported, the failure held"
case_ "5 code 2, no label"           false 2 false 1 "::error::a build's suite run failed (exit 2)"
case_ "6 code 2, label"              true  2 false 1 "::error::a build's suite run failed (exit 2)"
case_ "7 code 3, no label"           false 3 false 1 "::error::DIFFERENTIAL: incomplete recording: a recording is not a whole run"
case_ "8 code 3, label"              true  3 false 1 "::error::DIFFERENTIAL: incomplete recording: a recording is not a whole run"
case_ "9 code 1, label, fails on"    true  1 true  1 "::error::DIFFERENTIAL: 13 differences over 335 calls and 35 commands on a PR labelled refactor"
case_ "10 code 1, no label, fails on" false 1 true 0 "::notice::DIFFERENTIAL: 13 differences over 335 calls and 35 commands (no refactor label"
case_ "11 the label read fails"      fail  0 false 1 "::error::the PR's labels could not be read (gh pr view)"

if [ "$fails" -ne 0 ]; then echo "test_differential_step: $fails failed"; exit 1; fi
echo "test_differential_step: all cases ok"
