#!/usr/bin/env python3
"""The planted cases of tools/bench-compare's CI verdict (#71): each way a
PR could turn the gate off, and the cases it must let through. Run by the
CI job bench before the benches: python3 tools/test_bench_compare.py
"""

import copy
import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import unittest

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


def run(scale=1.0, commit="b"):
    return {"commit": commit * 40, "recorded": "2026-10-22", "benches": measure(scale)}


def runs(n):
    return [run() for _ in range(n)]


def entry(runs):
    return {"profile": KEY, "gate": True, "machine": MACHINE, "toolchain": TOOLCHAIN, "runs": runs}


def baseline(runs, **extra):
    return {"margin": 0.05, "profiles": {KEY: entry(runs)}, **extra}


def judge(tree, base, measured=None):
    lines, errors, regressions, gating, candidate = bc.judge(
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
        lines, errors, regressions, _, candidate = bc.judge(tree, base, measure(scale=1.5), CTX)
        self.assertEqual(len(regressions), 2, "compared at the base's margin, not the tree's")
        self.assertEqual(candidate["margin"], 0.05)
        self.assertEqual(len(candidate["profiles"][KEY]["runs"]), 6)


class WhatTheGateLetsThrough(unittest.TestCase):
    def test_no_entry_records_a_candidate(self):
        errors, regressions, gating, candidate = judge(bc.empty(), bc.empty())
        self.assertEqual((errors, regressions, gating), ([], [], False))
        self.assertEqual(candidate["profiles"][KEY]["runs"][0]["benches"], measure())
        self.assertEqual(candidate["profiles"][KEY]["runs"][0]["commit"], "c" * 40)

    def test_matching_opening_entry_passes(self):
        errors, _, gating, candidate = judge(baseline([run(scale=1.03)]), bc.empty())
        self.assertEqual(errors, [])
        self.assertFalse(gating)
        self.assertEqual(len(candidate["profiles"][KEY]["runs"]), 2)

    def test_matching_appended_run_passes(self):
        errors, *_ = judge(baseline(runs(4) + [run(scale=0.97)]), baseline(runs(4)))
        self.assertEqual(errors, [])

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


class TheJobsInputs(unittest.TestCase):
    def tool(self, *args, **env):
        e = {k: v for k, v in os.environ.items() if k not in ("BENCH_PROFILE", "BENCH_COMMIT")}
        e.update(env)
        return subprocess.run([sys.executable, TOOL, *args], env=e, capture_output=True, text=True)

    def test_ci_refuses_a_renamed_profile(self):
        r = self.tool("--ci", "--out", os.devnull, BENCH_PROFILE="ubuntu-fast")
        self.assertEqual(r.returncode, 1)
        self.assertIn("is refused", r.stderr)

    def test_ci_refuses_a_supplied_directory(self):
        r = self.tool("--ci", "--compare-only", HERE, "--out", os.devnull)
        self.assertIn("measures its own results", r.stderr)

    def test_ci_refuses_a_commit_that_is_not_the_checkout(self):
        r = self.tool("--ci", "--out", os.devnull, BENCH_COMMIT="f" * 40)
        self.assertIn("is not the checked-out commit", r.stderr)

    def test_a_gate_entry_is_never_recorded_by_hand(self):
        r = self.tool("--record", BENCH_PROFILE=bc.GATE_PROFILE)
        self.assertIn("is the CI job's measure", r.stderr)

    def test_the_constants(self):
        self.assertEqual((bc.GATE_PROFILE, bc.MARGIN, bc.MIN_CALLS, bc.MIN_RUNS),
                         ("ubuntu-latest", 0.05, 100, 5))


if __name__ == "__main__":
    unittest.main(verbosity=2)
