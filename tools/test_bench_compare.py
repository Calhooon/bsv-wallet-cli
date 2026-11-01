#!/usr/bin/env python3
"""The planted cases of tools/bench-compare's CI verdict (#71): each way a
PR could turn the gate off, and the cases it must let through. Run by the
CI job bench before the benches: python3 tools/test_bench_compare.py
"""

import contextlib
import copy
import io
import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "bench-compare")
loader = importlib.machinery.SourceFileLoader("bench_compare", TOOL)
spec = importlib.util.spec_from_loader("bench_compare", loader)
bc = importlib.util.module_from_spec(spec)
loader.exec_module(bc)

MACHINE = "AMD EPYC 7763 64-Core Processor, 4 cores"
KEY = bc.gate_key(MACHINE)
TOOLCHAIN = "rustc 1.98.1 (aaaaaaaaa 2026-09-01)"
CTX = {"key": KEY, "machine": MACHINE, "toolchain": TOOLCHAIN, "commit": "c" * 40,
       "run_id": "9", "recorded": "2026-10-23"}


def measure(scale=1.0, calls=400):
    return {
        "gate_assess/need/1000": {"p50_us": 3000.0 * scale, "p99_us": 5000.0 * scale, "calls": calls},
        "list_outputs/limit100/1000": {"p50_us": 700.0 * scale, "p99_us": 1200.0 * scale, "calls": calls},
    }


def at_p99(p99):
    """This run's measure with gate_assess's p99 at the number given."""
    m = measure()
    m["gate_assess/need/1000"]["p99_us"] = p99
    return m


def run(scale=1.0, commit="b"):
    return {"commit": commit * 40, "recorded": "2026-10-22", "benches": measure(scale)}


def runs(n):
    return [run() for _ in range(n)]


def entry(runs):
    return {"profile": KEY, "gate": True, "machine": MACHINE, "toolchain": TOOLCHAIN, "runs": runs}


def baseline(runs, **extra):
    return {"margin": 0.05, "profiles": {KEY: entry(runs)}, **extra}


def judge(tree, base, measured=None):
    lines, warnings, errors, regressions, gating, candidate = bc.judge(
        tree, base, measured or measure(), CTX)
    return errors, regressions, gating, candidate


class TheTreeCannotTurnTheGateOff(unittest.TestCase):
    def test_margin_change_fails(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["margin"] = 10
        errors, *_ = judge(tree, base)
        self.assertTrue(any("margin" in e for e in errors), errors)

    def test_margin_on_an_entry_fails(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["profiles"][KEY]["margin"] = 10
        errors, *_ = judge(tree, base)
        self.assertTrue(any("margin is the file's" in e for e in errors), errors)

    def test_margin_on_an_opening_file_fails(self):
        tree = baseline([run()])
        tree["margin"] = 10
        errors, *_ = judge(tree, bc.empty())
        self.assertTrue(any("margin" in e for e in errors), errors)

    def test_dropped_entry_fails(self):
        base = baseline(runs(5))
        errors, *_ = judge(bc.empty(), base)
        self.assertTrue(any("dropped" in e for e in errors), errors)

    def test_dropped_bench_id_fails(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        for r in tree["profiles"][KEY]["runs"]:
            del r["benches"]["gate_assess/need/1000"]
        errors, *_ = judge(tree, base)
        self.assertTrue(any("bench ids differ" in e for e in errors), errors)

    def test_bench_not_measured_fails(self):
        base = baseline(runs(5))
        m = measure()
        del m["gate_assess/need/1000"]
        errors, *_ = judge(base, base, m)
        self.assertTrue(any("not measured" in e for e in errors), errors)

    def test_inflated_opening_entry_fails_naming_the_gap(self):
        tree = baseline([run(scale=3.0)])
        errors, *_ = judge(tree, bc.empty())
        self.assertTrue(any("opening entry does not match" in e and "+200.0%" in e for e in errors), errors)

    def test_opening_entry_with_other_ids_fails(self):
        r = run()
        r["benches"]["made_up/1"] = {"p50_us": 1.0, "p99_us": 1.0, "calls": 400}
        errors, *_ = judge(baseline([r]), bc.empty())
        self.assertTrue(any("bench ids are not this run's" in e for e in errors), errors)

    def test_forged_history_in_a_opening_entry_fails(self):
        errors, *_ = judge(baseline(runs(5)), bc.empty())
        self.assertTrue(any("carries 5 runs" in e for e in errors), errors)

    def test_rewritten_run_fails(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["profiles"][KEY]["runs"][0] = run(scale=3.0)
        errors, *_ = judge(tree, base)
        self.assertTrue(any("never rewrites" in e for e in errors), errors)

    def test_inflated_appended_run_fails(self):
        base = baseline(runs(4))
        tree = baseline(runs(4) + [run(scale=3.0)])
        errors, *_ = judge(tree, base)
        self.assertTrue(any("appended run does not match" in e for e in errors), errors)

    def test_two_appended_runs_fail(self):
        base = baseline(runs(3))
        tree = baseline(runs(5))
        errors, *_ = judge(tree, base)
        self.assertTrue(any("appends 2 runs" in e for e in errors), errors)

    def test_renamed_profile_key_fails(self):
        base = baseline(runs(5))
        tree = {"margin": 0.05, "profiles": {"ubuntu-fast-x": base["profiles"][KEY]}}
        errors, *_ = judge(tree, base)
        self.assertTrue(any("a profile is" in e for e in errors), errors)
        self.assertTrue(any("dropped" in e for e in errors), errors)

    def test_entry_flipped_to_not_gate_fails(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["profiles"][KEY]["gate"] = False
        errors, *_ = judge(tree, base)
        self.assertTrue(any("gate true" in e for e in errors), errors)

    def test_too_few_calls_fail(self):
        base = baseline(runs(5))
        errors, *_ = judge(base, base, measure(calls=30))
        self.assertTrue(any("fewer than 100" in e for e in errors), errors)

    def test_a_failed_tree_is_not_the_candidate(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["margin"] = 10
        lines, warnings, errors, regressions, _, candidate = bc.judge(tree, base, measure(scale=1.5), CTX)
        self.assertEqual(len(regressions), 2, "compared at the base's margin, not the tree's")
        self.assertEqual(candidate["margin"], 0.05)
        self.assertEqual(len(candidate["profiles"][KEY]["runs"]), 6)

    def test_an_appended_slow_run_is_not_the_reference(self):
        # The PR appends a run 3x slower than the base's and the job measures
        # at that speed: the appended run matches this run's measure, and
        # the reference is still the base's runs.
        base = baseline(runs(bc.MIN_RUNS))
        tree = baseline(runs(bc.MIN_RUNS) + [run(scale=3.0)])
        lines, warnings, errors, regressions, gating, candidate = bc.judge(tree, base, measure(scale=3.0), CTX)
        self.assertTrue(any("over the base's reference" in e and "+200.0%" in e for e in errors), errors)
        self.assertTrue(gating)
        self.assertEqual(len(regressions), 2, lines)
        self.assertTrue(any(l.endswith(" FAIL") for l in lines), lines)
        self.assertEqual(len(candidate["profiles"][KEY]["runs"]), bc.MIN_RUNS + 1, "the base plus this run")

    def test_a_3x_fifth_run_append_fails_in_the_warn_only_period(self):
        # Four runs in the base: the measure is only over, but the fifth run
        # the PR commits would switch the gate on with it in the reference.
        base = baseline(runs(bc.MIN_RUNS - 1))
        tree = baseline(runs(bc.MIN_RUNS - 1) + [run(scale=3.0)])
        lines, warnings, errors, regressions, gating, _ = bc.judge(tree, base, measure(scale=3.0), CTX)
        self.assertTrue(any("appended run is over the base's reference" in e and "+200.0%" in e
                            for e in errors), errors)
        self.assertFalse(gating)
        self.assertEqual(len(regressions), 2)
        self.assertTrue(any(l.endswith(" over") for l in lines), lines)

    def test_a_10_percent_append_over_a_4_9_percent_measure_fails(self):
        # The measure is within the margin of the reference and the appended
        # run within the margin of the measure: the creep the ceiling stops.
        base = baseline(runs(bc.MIN_RUNS))
        tree = baseline(runs(bc.MIN_RUNS) + [run(scale=1.10)])
        errors, regressions, gating, _ = judge(tree, base, measure(scale=1.049))
        self.assertEqual((regressions, gating), ([], True))
        self.assertTrue(any("appended run is over the base's reference" in e and "+10.0%" in e
                            for e in errors), errors)
        self.assertFalse(any("does not match this run's measure" in e for e in errors), errors)

    def test_a_6_percent_re_record_has_no_error_and_a_warning_naming_6_percent(self):
        # The pin moved to a compiler 6 percent slower: the re-record is the
        # pin's cost, named in a warning with both toolchains, never an error.
        old = "rustc 1.97.0 (bbbbbbbbb 2026-08-01)"
        base = baseline(runs(bc.MIN_RUNS))
        base["profiles"][KEY]["toolchain"] = old
        tree = baseline([run(scale=1.06)])
        lines, warnings, errors, regressions, gating, _ = bc.judge(tree, base, measure(scale=1.06), CTX)
        self.assertEqual((errors, gating), ([], False), "not gated on a new pin, and no error")
        self.assertTrue(any("re-record" in w and "over the base's reference" in w and "+6.0%" in w
                            and old in w and TOOLCHAIN in w for w in warnings), warnings)

    def test_a_re_record_that_does_not_match_the_measure_still_fails(self):
        base = baseline(runs(bc.MIN_RUNS))
        base["profiles"][KEY]["toolchain"] = "rustc 1.97.0 (bbbbbbbbb 2026-08-01)"
        tree = baseline([run(scale=1.5)])
        errors, *_ = judge(tree, base, measure(scale=1.06))
        self.assertTrue(any("re-record" in e and "does not match this run's measure" in e for e in errors), errors)

    def test_5251_over_5000_fails_as_measure_and_as_appended_run(self):
        base = baseline(runs(bc.MIN_RUNS))
        measured = at_p99(5251.0)
        tree = baseline(runs(bc.MIN_RUNS) + [dict(run(), benches=measured)])
        errors, regressions, gating, _ = judge(tree, base, measured)
        self.assertTrue(gating)
        self.assertEqual(len(regressions), 1, regressions)
        self.assertTrue(any("appended run is over the base's reference" in e
                            and "5251.0" in e for e in errors), errors)

    def test_no_base_gates_nothing(self):
        tree = baseline(runs(bc.MIN_RUNS))
        lines, warnings, errors, regressions, gating, _ = bc.judge(tree, None, measure(scale=3.0), CTX)
        self.assertEqual(errors, [])
        self.assertFalse(gating)
        self.assertTrue(any("nothing is gated" in w for w in warnings), warnings)


class WhatTheGateLetsThrough(unittest.TestCase):
    def test_no_entry_records_a_candidate(self):
        errors, regressions, gating, candidate = judge(bc.empty(), bc.empty())
        self.assertEqual((errors, regressions, gating), ([], [], False))
        self.assertEqual(candidate["profiles"][KEY]["runs"][0]["benches"], measure())
        self.assertEqual(candidate["profiles"][KEY]["runs"][0]["commit"], "c" * 40)

    def test_a_cpu_with_no_entry_is_loud(self):
        other = "Intel Xeon Platinum 8370C, 4 cores"
        ctx = dict(CTX, machine=other, key=bc.gate_key(other))
        base = baseline(runs(bc.MIN_RUNS))
        lines, warnings, errors, regressions, gating, candidate = bc.judge(base, base, measure(scale=3.0), ctx)
        self.assertEqual((errors, regressions, gating), ([], [], False))
        self.assertTrue(any(other in w and "nothing is gated" in w for w in warnings), warnings)
        self.assertEqual(len(candidate["profiles"][ctx["key"]]["runs"]), 1, "the candidate opens the entry")
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as d:
            summary = os.path.join(d, "summary.md")
            out = io.StringIO()
            with contextlib.redirect_stdout(out), unittest.mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": summary}):
                bc.announce(warnings, "::")
            with open(summary) as f:
                written = f.read()
        self.assertIn(f"::warning::no gate entry for this runner's CPU ({other}", out.getvalue())
        self.assertIn(f"no gate entry for this runner's CPU ({other}", written)
        self.assertIn("nothing is gated", written)

    def test_matching_opening_entry_passes(self):
        errors, _, gating, candidate = judge(baseline([run(scale=1.03)]), bc.empty())
        self.assertEqual(errors, [])
        self.assertFalse(gating)
        self.assertEqual(len(candidate["profiles"][KEY]["runs"]), 1, "the base's file plus this run")

    def test_matching_appended_run_passes(self):
        errors, *_ = judge(baseline(runs(4) + [run(scale=0.97)]), baseline(runs(4)))
        self.assertEqual(errors, [])

    def test_an_append_within_the_margin_of_the_reference_passes(self):
        for n in (bc.MIN_RUNS - 1, bc.MIN_RUNS):
            errors, regressions, _, _ = judge(baseline(runs(n) + [run(scale=1.049)]), baseline(runs(n)),
                                              measure(scale=1.03))
            self.assertEqual((errors, regressions), ([], []))

    def test_5250_over_5000_is_at_the_margin_and_passes(self):
        # 5250 / 5000 - 1 is 0.050000000000000044: at the margin, not over it.
        base = baseline(runs(bc.MIN_RUNS))
        measured = at_p99(5250.0)
        tree = baseline(runs(bc.MIN_RUNS) + [dict(run(), benches=measured)])
        errors, regressions, gating, _ = judge(tree, base, measured)
        self.assertEqual((errors, regressions, gating), ([], [], True))
        # And as a committed run within the margin of this run's measure.
        errors, *_ = judge(tree, base, at_p99(5000.0))
        self.assertFalse(any("does not match this run's measure" in e for e in errors), errors)

    def test_warn_only_below_five_runs(self):
        base = baseline(runs(4))
        errors, regressions, gating, _ = judge(base, base, measure(scale=1.5))
        self.assertEqual(errors, [])
        self.assertEqual(len(regressions), 2)
        self.assertFalse(gating)

    def test_gates_at_five_runs(self):
        base = baseline(runs(5))
        errors, regressions, gating, _ = judge(base, base, measure(scale=1.5))
        self.assertEqual(errors, [])
        self.assertTrue(gating)
        self.assertEqual(len(regressions), 2)
        errors, regressions, gating, _ = judge(base, base, measure(scale=1.04))
        self.assertEqual((errors, regressions, gating), ([], [], True))

    def test_reference_is_the_highest_run(self):
        base = baseline(runs(4) + [run(scale=1.2)])
        _, regressions, gating, _ = judge(base, base, measure(scale=1.2))
        self.assertTrue(gating)
        self.assertEqual(regressions, [])

    def test_another_cpus_entry_unchanged_passes(self):
        other = bc.gate_key("Intel Xeon Platinum 8370C, 4 cores")
        base = baseline(runs(5))
        base["profiles"][other] = dict(entry([run()]), profile=other, machine="Intel Xeon Platinum 8370C, 4 cores")
        errors, *_ = judge(base, base)
        self.assertEqual(errors, [])

    def test_developer_entries_are_not_read(self):
        base = baseline(runs(5))
        tree = copy.deepcopy(base)
        tree["profiles"]["dev-apple-m1-max-10-cores"] = {"anything": True}
        errors, *_ = judge(tree, base)
        self.assertEqual(errors, [])


def job(workflow, name):
    """A job's text in a workflow of .github/workflows: from its key to the
    comment that opens the next job (or the file's end)."""
    with open(os.path.join(os.path.dirname(HERE), ".github", "workflows", workflow)) as f:
        text = f.read()
    return text.split(f"\n  {name}:\n", 1)[1].split("\n  # ", 1)[0]


class TheJobsInputs(unittest.TestCase):
    def tool(self, *args, rustc=None, **env):
        """The tool run as the job runs it; rustc, when named, is what a
        rustc on the PATH ahead of the toolchain's says to -V."""
        e = {k: v for k, v in os.environ.items() if k not in ("BENCH_PROFILE", "BENCH_COMMIT")}
        e.update(env)
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as d:
            if rustc is not None:
                fake = os.path.join(d, "rustc")
                with open(fake, "w") as f:
                    f.write(f"#!/bin/sh\necho '{rustc}'\n")
                os.chmod(fake, 0o755)
                e["PATH"] = d + os.pathsep + e.get("PATH", "")
            return subprocess.run([sys.executable, TOOL, *args], env=e, capture_output=True, text=True)

    def test_ci_refuses_a_toolchain_off_the_pin(self):
        # A PR's rust-toolchain.toml moves rustc -V in the checkout.
        r = self.tool("--ci", "--out", os.devnull, rustc="rustc 1.99.0 (ccccccccc 2026-11-01)")
        self.assertEqual(r.returncode, 1)
        self.assertIn("--ci runs on rustc 1.98.1", r.stderr)
        self.assertIn("rustc 1.99.0", r.stderr)

    def test_the_pin_is_the_jobs(self):
        # The bench job is bench.yml's since 7b32844 (it left ci.yml for a
        # dispatch); the ratchet job is ci.yml's.
        bench = job("bench.yml", "bench")
        ratchet = job("ci.yml", "ratchet")
        self.assertIn(f"RUSTUP_TOOLCHAIN: {bc.TOOLCHAIN_PIN}\n", bench)
        self.assertIn(f"toolchain: {bc.TOOLCHAIN_PIN}\n", bench)
        self.assertIn(f"toolchain: {bc.TOOLCHAIN_PIN}\n", ratchet)

    def test_ci_refuses_a_renamed_profile(self):
        r = self.tool("--ci", "--out", os.devnull, BENCH_PROFILE="ubuntu-fast")
        self.assertEqual(r.returncode, 1)
        self.assertIn("is refused", r.stderr)

    def test_ci_refuses_a_supplied_directory(self):
        r = self.tool("--ci", "--compare-only", HERE, "--out", os.devnull)
        self.assertIn("measures its own results", r.stderr)

    def test_ci_refuses_a_commit_that_is_not_the_checkout(self):
        r = self.tool("--ci", "--out", os.devnull, rustc=TOOLCHAIN, BENCH_COMMIT="f" * 40)
        self.assertIn("is not the checked-out commit", r.stderr)

    def test_a_gate_entry_is_never_recorded_by_hand(self):
        r = self.tool("--record", BENCH_PROFILE=bc.GATE_PROFILE)
        self.assertIn("is the CI job's measure", r.stderr)

    def test_the_constants(self):
        self.assertEqual((bc.GATE_PROFILE, bc.MARGIN, bc.MIN_CALLS, bc.MIN_RUNS),
                         ("ubuntu-latest", 0.05, 100, 5))


if __name__ == "__main__":
    unittest.main(verbosity=2)
