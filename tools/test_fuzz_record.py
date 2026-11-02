#!/usr/bin/env python3
"""tools/fuzz-record's record and refusals (#445): the captured excerpt of
Nightly run 41413946663 (tools/testdata/fuzz-record/41413946663: its five
fuzz jobs' log lines, verbatim, and its artifact listing) gives the exact
record, and each way a night is not one record is refused by name.
Run by CI's scoreboard step: python3 -B tools/test_fuzz_record.py
"""

import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "fuzz-record")
FIXTURE = os.path.join(HERE, "testdata", "fuzz-record", "41413946663")
loader = importlib.machinery.SourceFileLoader("fuzz_record", TOOL)
spec = importlib.util.spec_from_loader("fuzz_record", loader)
fr = importlib.util.module_from_spec(spec)
loader.exec_module(fr)

TARGETS = ["beef_reader", "brc29_remittance", "served_json", "explorer_bodies", "allow_pending"]

# The record of run 41413946663, as committed in the PR that landed the tool.
RECORD = """{
  "schema": 1,
  "commit": "2ca0173029cecefe50526b23669cfc9c2ad70cd7",
  "run": "https://github.com/Calhooon/bsv-wallet-cli/actions/runs/41413946663",
  "date": "2026-11-02",
  "targets": {
    "allow_pending": {"seconds": 3601, "executions": 149265139, "corpus_added": 3640, "crashes": 0},
    "beef_reader": {"seconds": 3601, "executions": 12580902, "corpus_added": 781, "crashes": 0},
    "brc29_remittance": {"seconds": 3601, "executions": 2132718, "corpus_added": 411, "crashes": 0},
    "explorer_bodies": {"seconds": 3601, "executions": 12430, "corpus_added": 379, "crashes": 0},
    "served_json": {"seconds": 3601, "executions": 117285867, "corpus_added": 6211, "crashes": 0}
  }
}
"""


def night():
    return fr.read(FIXTURE)


def job(n, target):
    return next(j for j in n["run"]["jobs"] if j["name"] == f"Fuzz ({target})")


def refused(n, *patterns):
    with unittest.TestCase().assertRaises(fr.Refused) as cm:
        fr.record(n, TARGETS)
    text = str(cm.exception)
    for p in patterns:
        if p not in text:
            raise AssertionError(f"{p!r} not in the refusal: {text}")
    return text


class TheRecord(unittest.TestCase):
    def test_the_captured_night_is_the_exact_record(self):
        rec, said = fr.record(night(), TARGETS)
        self.assertEqual(fr.render(rec), RECORD)
        self.assertEqual(len(said), 5)
        self.assertEqual(said[3], "explorer_bodies: 3601 s, 12430 executions, 379 corpus added, 0 crashes "
                                  "(job 125173162862, at 2ca0173, rustc 1.101.0-nightly (92b5b8c87 2026-10-29))")

    def test_the_command_writes_the_record_and_prints_each_target(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as d:
            out = os.path.join(d, "fuzz.json")
            r = subprocess.run([sys.executable, TOOL, "--from", FIXTURE, "--out", out],
                               capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            with open(out) as f:
                self.assertEqual(f.read(), RECORD)
            for t in TARGETS:
                self.assertIn(f"\n{t}: ", "\n" + r.stdout)
            self.assertIn("2ca0173029cecefe50526b23669cfc9c2ad70cd7, 2026-11-02, 5 targets", r.stdout)

    def test_the_steps_script_text_is_never_output(self):
        # The fuzz step's `echo "crashed=true"` and the merge step's
        # `echo "corpus additions: $(..)"` are echoed into every log.
        n = night()
        self.assertIn('echo "crashed=true"', n["logs"]["beef_reader"])
        self.assertIn('echo "corpus additions: $(find', n["logs"]["beef_reader"])
        rec, _ = fr.record(n, TARGETS)
        self.assertEqual(rec["targets"]["beef_reader"]["crashes"], 0)

    def test_a_bare_log_reads_as_ghs(self):
        n = night()
        n["logs"]["beef_reader"] = "\n".join(line.split("\t", 2)[2] for line in n["logs"]["beef_reader"].splitlines())
        self.assertEqual(fr.render(fr.record(n, TARGETS)[0]), RECORD)

    def test_a_crash_is_counted_from_the_artifact_and_witnessed_by_the_log(self):
        n = night()
        n["artifacts"]["crash-beef_reader"] = {"files": 2, "inputs": ["crash-0a1b"]}
        n["logs"]["beef_reader"] += ("\nFuzz (beef_reader)\tUNKNOWN STEP\t2026-11-02T09:00:00.0000000Z "
                                     "Test unit written to crashes/beef_reader/crash-0a1b\n")
        rec, _ = fr.record(n, TARGETS)
        self.assertEqual(rec["targets"]["beef_reader"]["crashes"], 1)

    def test_the_targets_are_the_manifests(self):
        self.assertEqual(sorted(fr.targets()), sorted(TARGETS))


class TheRefusals(unittest.TestCase):
    def test_a_job_not_success_is_refused_by_name(self):
        n = night()
        job(n, "served_json")["conclusion"] = "failure"
        refused(n, "served_json: the job 125173162645 concluded 'failure', not success")

    def test_a_job_on_another_commit_is_refused_by_name(self):
        n = night()
        n["logs"]["beef_reader"] = n["logs"]["beef_reader"].replace("HEAD is now at 2ca0173 ", "HEAD is now at 6bf9a2c ")
        refused(n, "a night is one tree", "beef_reader at 6bf9a2c")

    def test_jobs_on_two_commits_are_each_named(self):
        n = night()
        for t, sha in (("beef_reader", "6bf9a2c"), ("allow_pending", "45fecea")):
            n["logs"][t] = n["logs"][t].replace("HEAD is now at 2ca0173 ", f"HEAD is now at {sha} ")
        refused(n, "allow_pending at 45fecea, beef_reader at 6bf9a2c")

    def test_a_log_of_two_checkouts_is_refused(self):
        n = night()
        n["logs"]["allow_pending"] += "\n2026-11-02T09:00:00.0Z HEAD is now at 6bf9a2c other\n"
        refused(n, "allow_pending: its log names 2 checkouts")

    def test_two_rustc_are_refused(self):
        n = night()
        n["logs"]["served_json"] = n["logs"]["served_json"].replace("92b5b8c87 2026-10-29", "f9ceb4a4d 2026-10-31")
        refused(n, "the jobs' rustc differ", "served_json rustc 1.101.0-nightly (f9ceb4a4d 2026-10-31)")

    def test_a_missing_target_job_is_refused(self):
        n = night()
        n["run"]["jobs"] = [j for j in n["run"]["jobs"] if j["name"] != "Fuzz (allow_pending)"]
        refused(n, "are not the targets of fuzz/Cargo.toml")

    def test_another_workflow_is_refused(self):
        n = night()
        n["run"]["workflowName"] = "Differential"
        refused(n, "not Nightly")

    def test_a_missing_done_is_refused(self):
        n = night()
        n["logs"]["brc29_remittance"] = n["logs"]["brc29_remittance"].replace("Done 2132718 runs", "Done-")
        refused(n, "brc29_remittance: 0 lines of `Done <runs> runs in <s> second(s)`")

    def test_two_dones_are_refused(self):
        n = night()
        n["logs"]["brc29_remittance"] += "\nDone 5 runs in 1 second(s)\n"
        refused(n, "brc29_remittance: 2 lines of `Done")

    def test_done_against_the_stat_is_refused(self):
        n = night()
        n["logs"]["explorer_bodies"] = n["logs"]["explorer_bodies"].replace(
            "stat::number_of_executed_units: 12430", "stat::number_of_executed_units: 12431")
        refused(n, "explorer_bodies: `Done` names 12430 runs and `stat::number_of_executed_units` 12431")

    def test_additions_against_the_artifact_are_refused(self):
        n = night()
        n["artifacts"]["corpus-explorer_bodies"]["files"] = 378
        refused(n, "explorer_bodies: its log names 379 corpus additions and the artifact "
                   "corpus-explorer_bodies holds 378 files")

    def test_no_corpus_artifact_is_zero_files(self):
        n = night()
        del n["artifacts"]["corpus-explorer_bodies"]
        refused(n, "corpus-explorer_bodies holds 0 files")

    def test_a_crash_input_the_log_did_not_write_is_refused(self):
        n = night()
        n["artifacts"]["crash-allow_pending"] = {"files": 2, "inputs": ["oom-ff"]}
        refused(n, "allow_pending: the artifact crash-allow_pending holds the inputs ['oom-ff']")

    def test_an_artifact_of_no_target_is_refused(self):
        n = night()
        n["artifacts"]["corpus-retired"] = {"files": 1, "inputs": []}
        refused(n, "corpus-retired: an artifact of no target")

    def test_every_fault_is_named_at_once(self):
        n = night()
        job(n, "served_json")["conclusion"] = "cancelled"
        n["artifacts"]["corpus-beef_reader"]["files"] = 1
        refused(n, "served_json: the job", "beef_reader: its log names 781")

    def test_a_refusal_writes_nothing(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as d:
            bad = os.path.join(d, "night")
            subprocess.run(["cp", "-R", FIXTURE, bad], check=True)
            n = fr.read(bad)
            job(n, "beef_reader")["conclusion"] = "failure"
            with open(os.path.join(bad, "run.json"), "w") as f:
                fr.json.dump(n["run"], f)
            out = os.path.join(d, "fuzz.json")
            r = subprocess.run([sys.executable, TOOL, "--from", bad, "--out", out], capture_output=True, text=True)
            self.assertEqual(r.returncode, 1)
            self.assertIn("refused, nothing written: beef_reader: the job", r.stderr)
            self.assertFalse(os.path.exists(out))

    def test_the_arguments(self):
        for args, said in ((["--from", FIXTURE, "41413946663"], "one of the two"),
                           ([], "one of the two"),
                           (["--from", FIXTURE, "--keep", FIXTURE], "--keep is a run id's"),
                           (["41413946663", "--keep", FIXTURE], "it is a new directory"),
                           (["--record"], "unknown argument --record")):
            r = subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True)
            self.assertEqual(r.returncode, 1, args)
            self.assertIn(said, r.stderr, args)


if __name__ == "__main__":
    unittest.main(verbosity=2)
