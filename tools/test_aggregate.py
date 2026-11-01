#!/usr/bin/env python3
"""The planted cases of tools/lanes/aggregate.py (#390): the consensus rule of
docs/lanes/ENSEMBLE.md (three of three; two of three at a mean of 0.7 and not
at 0.69; one file never), a VERDICT counted over its DECISION, and each
malformed block exits 2 naming its file and field. Each case writes its files
in a directory of its own under $TMPDIR. Run by the CI job ratchet:
python3 tools/test_aggregate.py
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "lanes", "aggregate.py")


def block(decision, confidence, verdict=None, risks="RISKS: none", lead="REPORT: STATUS done\nbase abc\n"):
    head = f"VERDICT {verdict}\n" if verdict else ""
    return (f"{lead}\n{head}DECISION: {decision}\nCONFIDENCE: {confidence}\n{risks}\n"
            "RATIONALE: the proofs' tails read as claimed.\n")


class Case(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="test-aggregate-")

    def tearDown(self):
        shutil.rmtree(self.dir)

    def run_tool(self, *texts):
        paths = []
        for number, text in enumerate(texts):
            path = os.path.join(self.dir, f"reader-{number}.txt")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(text)
            paths.append(path)
        r = subprocess.run([sys.executable, "-B", TOOL, *paths], capture_output=True, text=True)
        return r.returncode, r.stdout, r.stderr

    def summary(self, out):
        return [line for line in out.splitlines() if line.startswith("AGGREGATE:")][-1]

    def test_three_of_three_merge(self):
        code, out, _ = self.run_tool(*(block("land it", c, "merge") for c in ("0.9", "0.8", "0.6")))
        self.assertEqual(code, 0, out)
        self.assertEqual(self.summary(out), "AGGREGATE: consensus agreement=3/3 mean=0.77 decision: merge")

    def test_two_of_three_at_mean_point_seven_is_consensus(self):
        code, out, _ = self.run_tool(block("land it", "0.8", "merge"), block("land it", "0.8", "merge"),
                                     block("pin is stale", "0.5", "fix"))
        self.assertEqual(code, 0, out)
        self.assertEqual(self.summary(out), "AGGREGATE: consensus agreement=2/3 mean=0.70 decision: merge")

    def test_two_of_three_at_mean_point_six_nine_is_split(self):
        code, out, _ = self.run_tool(block("land it", "0.8", "merge"), block("land it", "0.8", "merge"),
                                     block("pin is stale", "0.47", "fix"))
        self.assertEqual(code, 1, out)
        self.assertEqual(self.summary(out), "AGGREGATE: split agreement=2/3 mean=0.69 decision: merge")

    def test_missing_confidence_exits_two(self):
        text = "DECISION: the row stands\nRISKS: none\nRATIONALE: the BRC says so.\n"
        code, out, err = self.run_tool(block("x", "0.9"), text)
        self.assertEqual(code, 2, out)
        self.assertIn("reader-1.txt: missing CONFIDENCE", err)

    def test_confidence_outside_unit_interval_exits_two(self):
        for bad in ("1.2", "-0.1", "high"):
            code, out, err = self.run_tool(block("x", bad))
            self.assertEqual(code, 2, (bad, out))
            self.assertIn("reader-0.txt: CONFIDENCE must be a number in [0,1]", err)

    def test_verdict_fix_with_high_confidence_counts_as_fix(self):
        code, out, _ = self.run_tool(block("looks fine overall", "0.95", "fix"),
                                     block("looks fine overall", "0.9", "merge"),
                                     block("looks fine overall", "0.9", "fix"))
        self.assertEqual(code, 0, out)
        self.assertEqual(self.summary(out), "AGGREGATE: consensus agreement=2/3 mean=0.92 decision: fix")

    def test_one_file_is_never_a_consensus(self):
        code, out, _ = self.run_tool(block("land it", "1.0", "merge"))
        self.assertEqual(code, 1, out)
        self.assertEqual(self.summary(out), "AGGREGATE: split agreement=1/1 mean=1.00 decision: merge")

    def test_the_last_block_is_read(self):
        early = block("the template", "0.1")
        code, out, _ = self.run_tool(early + "\n" + block("The   Row  Stands", "0.9"),
                                     block("the row stands", "0.8"))
        self.assertEqual(code, 0, out)
        self.assertIn("decision: the row stands", self.summary(out))

    def test_risks_carry_their_scores(self):
        risks = "RISKS:\nthe upstream fix #1451 may move the row 0.3\n- explorer outage (0.1)"
        code, out, _ = self.run_tool(block("x", "0.9", risks=risks), block("x", "0.9"))
        self.assertEqual(code, 0, out)
        self.assertIn("risk 0.30 the upstream fix #1451 may move the row", out)
        self.assertIn("risk 0.10 explorer outage (", out)

    def test_a_risk_without_its_score_exits_two(self):
        code, _, err = self.run_tool(block("x", "0.9", risks="RISKS: the upstream fix may move"))
        self.assertEqual(code, 2)
        self.assertIn("reader-0.txt: RISKS line without its own score", err)

    def test_a_long_rationale_exits_two(self):
        text = block("x", "0.9").replace("RATIONALE: ", "RATIONALE:\n1\n2\n3\n4\n5\n")
        code, _, err = self.run_tool(text)
        self.assertEqual(code, 2)
        self.assertIn("reader-0.txt: RATIONALE must be one to 5 lines", err)


if __name__ == "__main__":
    unittest.main(verbosity=2)
