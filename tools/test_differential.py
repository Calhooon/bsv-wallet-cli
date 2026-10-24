#!/usr/bin/env python3
"""tools/differential's own tests (#70): each planted case of the cold review.

    python3 tools/test_differential.py        # every case
    python3 tools/test_differential.py -v     # each by name

Synthetic recordings, built here, stand for the suite's: the comparison is the
tool's own code (tools/differential loaded as a module), and the recording
side is tests/e2e/lib/record.js run under node against a stand-in binary.
The standard library only, like the tool.
"""

import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

# The tool has no .py name; loading it must leave no __pycache__ in tools/.
sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent.parent
_loader = importlib.machinery.SourceFileLoader("differential", str(ROOT / "tools" / "differential"))
_spec = importlib.util.spec_from_loader("differential", _loader)
D = importlib.util.module_from_spec(_spec)
_loader.exec_module(D)

KEYS = ["11" * 32, "22" * 32, "33" * 32, "44" * 32]


class RecordingHoldsNoKey(unittest.TestCase):
    """Finding 1: the four root keys never reach a recording."""

    def test_init_key_is_named_by_seat(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            # A stand-in binary that echoes its argv to stdout and stderr, as
            # a careless one might; and fails when asked to.
            bin_ = tmp / "bsv-wallet"
            bin_.write_text('#!/bin/sh\necho "args: $*"\necho "args: $*" >&2\n[ "$1" = fail ] && exit 3\nexit 0\n')
            bin_.chmod(0o755)
            rec = tmp / "rec.jsonl"
            wallet = tmp / "w"
            wallet.mkdir()
            script = f"""
const record = require({json.dumps(str(ROOT / 'tests/e2e/lib/record.js'))});
record.install({json.dumps(str(bin_))});
const {{ execSync }} = require('child_process');
const dir = {json.dumps(str(wallet))};
record.walletDir('wallet-A', dir);
for (const seat of ['wallet-A', 'wallet-B', 'wallet-C', 'funder']) {{
  execSync(`cd "${{dir}}" && "{bin_}" init --key ${{record.rootKeyFor(seat)}}`, {{ stdio: 'pipe' }});
}}
try {{ execSync(`cd "${{dir}}" && "{bin_}" fail --key ${{record.rootKeyFor('wallet-A')}}`, {{ stdio: 'pipe' }}); }}
catch (e) {{ if (e.message.includes({json.dumps(KEYS[0])})) {{ console.log('LEAK in error'); process.exit(1); }} }}
"""
            env = {**os.environ, "DIFF_RECORD": str(rec), "DIFF_ROOT_KEYS": ",".join(KEYS), "E2E_HERMETIC": "1"}
            r = subprocess.run(["node", "-e", script], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            text = rec.read_text()
            for k in KEYS:
                self.assertNotIn(k, text)
            events = [json.loads(line) for line in text.splitlines()]
            self.assertEqual(events[0]["argv"], ["init", "--key", "<root-key:A>"])
            self.assertEqual([e["argv"][2] for e in events[:4]],
                             ["<root-key:A>", "<root-key:B>", "<root-key:C>", "<root-key:funder>"])


if __name__ == "__main__":
    unittest.main()
