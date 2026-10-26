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
SCENARIOS = D.scenario_names()


def varint(n):
    return bytes([n]) if n < 0xFD else b"\xfd" + n.to_bytes(2, "little")


def push(data):
    return varint(len(data)) + data if len(data) <= 0x4B else b"\x4c" + bytes([len(data)]) + data


def der(r=None, s=None):
    """A strict DER signature of a random r (its high bit set or not) and a low s."""
    # No leading zero byte in either draw: strict DER has no excess padding.
    r = r if r is not None else bytes([os.urandom(1)[0] | 0x01]) + os.urandom(31)
    s = s if s is not None else bytes([(os.urandom(1)[0] & 0x3F) | 0x01]) + os.urandom(31)
    r = b"\x00" + r if r[0] & 0x80 else r
    body = b"\x02" + bytes([len(r)]) + r + b"\x02" + bytes([len(s)]) + s
    return b"\x30" + bytes([len(body)]) + body


def pubkey():
    return b"\x02" + os.urandom(32)


def raw_tx(inputs, outputs):
    """inputs: (prev txid display bytes, vout, scriptSig); outputs: (satoshis, script)."""
    b = (1).to_bytes(4, "little") + varint(len(inputs))
    for prev, vout, script in inputs:
        b += prev[::-1] + vout.to_bytes(4, "little") + varint(len(script)) + script + b"\xff\xff\xff\xff"
    b += varint(len(outputs))
    for sats, script in outputs:
        b += sats.to_bytes(8, "little") + varint(len(script)) + script
    return b + bytes(4)


def p2pkh():
    return b"\x76\xa9\x14" + os.urandom(20) + b"\x88\xac"


def signed_tx(sighash=0x41, extra=b"", key=None):
    """A one-input P2PKH spend over a random change coin, as a run draws it."""
    sig = push(der() + bytes([sighash])) + push(key or pubkey()) + extra
    return raw_tx([(os.urandom(32), 1, sig)], [(1000, p2pkh()), (4321, p2pkh())]).hex()


def recording(response=None, request=None):
    """A complete recording: four wallets and each phase's calls; the planted value in the first scenario."""
    ev = []

    def add(**e):
        ev.append({"seq": len(ev), **e})

    for seat in ("funder", "A", "B", "C"):
        add(kind="wallet", phase="setup", seat=seat, dir=f"/w/{seat}-{os.urandom(3).hex()}", port=1, identityKey="02" * 33)
        add(kind="cli", phase="setup", seat=seat, argv=["init", "--key", f"<root-key:{seat}>"], exit=0, signal=None, stdout="", stderr="")
    for name in SCENARIOS + ["sweep"]:
        add(kind="http", phase=name, seat="A", method="POST", path="/createAction",
            request=request if name == SCENARIOS[0] else {"description": name}, status=200,
            response=response if name == SCENARIOS[0] else {"ok": True})
        add(kind="cli", phase=name, seat="A", argv=["balance"], exit=0, signal=None, stdout="1000\n", stderr="")
    return ev


def compare(a, b):
    """The tool's comparison of two recordings in memory: (report lines, differences)."""
    lines, count, *_, gaps = D.compare_events([D._hexify(e) for e in a], [D._hexify(e) for e in b], SCENARIOS)
    assert not gaps, gaps
    return lines, count


def run_compare(a, b):
    """`tools/differential --compare` on two recordings written as files: (exit code, stdout)."""
    with tempfile.TemporaryDirectory() as tmp:
        files = []
        for name, events in (("a", a), ("b", b)):
            f = Path(tmp) / f"{name}.jsonl"
            f.write_text(events if isinstance(events, str) else "".join(json.dumps(e) + "\n" for e in events))
            files.append(str(f))
        r = subprocess.run([sys.executable, str(ROOT / "tools" / "differential"), "--compare", *files],
                           capture_output=True, text=True)
        return r.returncode, r.stdout


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


class ScriptSigPushes(unittest.TestCase):
    """Finding 2: an input script is its pushes, not one opaque atom."""

    def test_floor_two_draws_are_equal(self):
        lines, count = compare(recording({"tx": signed_tx()}), recording({"tx": signed_tx()}))
        self.assertEqual(count, 0, lines)

    def test_planted_sighash_c1_is_caught(self):
        # The review's plant: every input signed with sighash 0xC1 against a 0x41 preimage.
        lines, count = compare(recording({"tx": signed_tx(0x41)}), recording({"tx": signed_tx(0xC1)}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".script.pushes[0].sighash", lines[1])
        self.assertIn('base "41"', lines[1])
        self.assertIn('head "c1"', lines[1])

    def test_planted_push_count_is_caught(self):
        lines, count = compare(recording({"tx": signed_tx()}), recording({"tx": signed_tx(extra=push(b"\x01"))}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".script.pushes.length", lines[1])

    def test_planted_pubkey_length_is_caught(self):
        long_key = b"\x04" + os.urandom(64)
        lines, count = compare(recording({"tx": signed_tx()}), recording({"tx": signed_tx(key=long_key)}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".script.pushes[1].", lines[1])

    def test_planted_high_s_is_caught(self):
        high = signed_tx()
        # The same draw with s = n - 1 (high, still strict DER).
        s = (D.SECP256K1_N - 1).to_bytes(32, "big")
        sig = push(der(s=b"\x00" + s) + b"\x41") + push(pubkey())
        high = raw_tx([(os.urandom(32), 1, sig)], [(1000, p2pkh()), (4321, p2pkh())]).hex()
        lines, count = compare(recording({"tx": signed_tx()}), recording({"tx": high}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".lowS", lines[1])


class LengthsInPlaceholders(unittest.TestCase):
    """Finding 3: a renamed reference, derivation or ciphertext keeps its decoded length."""

    def b64(self, n):
        import base64
        return base64.b64encode(os.urandom(n)).decode()

    def test_floor_same_lengths_are_equal(self):
        a = recording({"reference": self.b64(12), "ciphertext": os.urandom(80).hex()}, {"derivationPrefix": self.b64(16)})
        b = recording({"reference": self.b64(12), "ciphertext": os.urandom(80).hex()}, {"derivationPrefix": self.b64(16)})
        lines, count = compare(a, b)
        self.assertEqual(count, 0, lines)

    def test_planted_reference_grown_by_four_characters_is_caught(self):
        ref = self.b64(12)
        grown = ref + self.b64(3)  # four characters more, three bytes
        self.assertEqual(len(grown), len(ref) + 4)
        lines, count = compare(recording({"reference": ref}), recording({"reference": grown}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".response.reference", lines[1])
        self.assertIn("/12B>", lines[1])
        self.assertIn("/15B>", lines[1])

    def test_planted_derivation_length_is_caught(self):
        lines, count = compare(recording(None, {"derivationPrefix": self.b64(16)}),
                               recording(None, {"derivationPrefix": self.b64(32)}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".request.derivationPrefix", lines[1])

    def test_planted_ciphertext_length_is_caught(self):
        lines, count = compare(recording({"ciphertext": os.urandom(80).hex()}),
                               recording({"ciphertext": os.urandom(81).hex()}))
        self.assertEqual(count, 1, lines)
        self.assertIn("/80B>", lines[1])


def without(events, drop):
    return [e for e in events if not drop(e)]


class FailClosed(unittest.TestCase):
    """Finding 4: a recording that is not a whole run is exit 3, never 0 and never 1."""

    def assertIncomplete(self, a, b, why):
        code, out = run_compare(a, b)
        self.assertEqual(code, 3, out)
        self.assertIn("DIFFERENTIAL: INCOMPLETE recording", out)
        self.assertIn(why, out)

    def test_whole_and_equal_is_0(self):
        code, out = run_compare(recording({"ok": 1}), recording({"ok": 1}))
        self.assertEqual(code, 0, out)

    def test_whole_with_a_difference_is_1(self):
        code, out = run_compare(recording({"ok": 1}), recording({"ok": 2}))
        self.assertEqual(code, 1, out)

    def test_planted_two_empty_recordings(self):
        self.assertIncomplete("", "", "base: no events")

    def test_planted_no_commands(self):
        r = without(recording(), lambda e: e["kind"] == "cli")
        self.assertIncomplete(r, r, "no commands")

    def test_planted_no_http_calls(self):
        r = without(recording(), lambda e: e["kind"] == "http")
        self.assertIncomplete(r, r, "no HTTP calls")

    def test_planted_no_wallet_events(self):
        r = without(recording(), lambda e: e["kind"] == "wallet")
        self.assertIncomplete(r, r, "no wallet events")

    def test_planted_truncated_mid_line(self):
        text = "".join(json.dumps(e) + "\n" for e in recording())
        self.assertIncomplete(recording(), text[: len(text) - 40], "head: line")

    def test_planted_truncated_at_a_line(self):
        r = recording()
        self.assertIncomplete(r, r[: len(r) // 2], "head: no sweep phase")

    def test_planted_one_call_fewer(self):
        r = recording()
        fewer = [e for i, e in enumerate(r) if not (e["phase"] == SCENARIOS[5] and e["kind"] == "cli")]
        self.assertIncomplete(r, fewer, f"phase {SCENARIOS[5]}: base 2 calls and commands, head 1")

    def test_planted_no_setup_phase(self):
        r = without(recording(), lambda e: e["phase"] == "setup")
        self.assertIncomplete(r, r, "no setup phase")

    def test_planted_no_sweep_phase(self):
        r = without(recording(), lambda e: e["phase"] == "sweep")
        self.assertIncomplete(r, r, "no sweep phase")

    def test_planted_a_scenario_missing(self):
        r = without(recording(), lambda e: e["phase"] == SCENARIOS[10])
        self.assertIncomplete(r, r, f"23 scenario phases, the suite has {len(SCENARIOS)}")

    def test_planted_a_phase_the_suite_lacks(self):
        r = recording()
        r.append({**r[-1], "seq": len(r), "phase": "a-25th-scenario"})
        self.assertIncomplete(r, r, "a phase the suite does not have: a-25th-scenario")

    def test_the_suite_has_24(self):
        self.assertEqual(len(SCENARIOS), 24)


class LogRulesOnlyOnStderr(unittest.TestCase):
    """Finding 7: the clock and length rules of cli.stderr touch nothing else; revelationTime is its own."""

    def test_floor_stderr_clock_and_lengths(self):
        a, b = recording(), recording()
        a[-1]["stderr"] = "2026-10-24T03:45:52.008Z  INFO explorer_call ms=12 bytes=4021\n"
        b[-1]["stderr"] = "2026-10-24T03:46:01.311Z  INFO explorer_call ms=9 bytes=4022\n"
        lines, count = compare(a, b)
        self.assertEqual(count, 0, lines)

    def test_planted_a_time_in_a_response_is_caught(self):
        lines, count = compare(recording({"createdAt": "2026-10-24T03:45:52.008Z"}),
                               recording({"createdAt": "2026-10-24T03:46:52.008Z"}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".response.createdAt", lines[1])

    def test_planted_ms_in_stdout_is_caught(self):
        a, b = recording(), recording()
        a[-1]["stdout"], b[-1]["stdout"] = "took ms=12 bytes=40\n", "took ms=13 bytes=41\n"
        lines, count = compare(a, b)
        self.assertEqual(count, 1, lines)
        self.assertIn(".stdout", lines[1])

    def test_revelation_time_is_its_own_entry(self):
        lines, count = compare(recording({"revelationTime": "2026-10-24T03:45:52.008Z"}),
                               recording({"revelationTime": "2026-10-24T03:47:00.100Z"}))
        self.assertEqual(count, 0, lines)
        self.assertTrue(any("revelationTime" in field for field, _ in D.EXCLUSIONS))

    def test_planted_revelation_time_form_is_caught(self):
        lines, count = compare(recording({"revelationTime": "2026-10-24T03:45:52.008Z"}),
                               recording({"revelationTime": "1761277552008"}))
        self.assertEqual(count, 1, lines)
        self.assertIn(".response.revelationTime", lines[1])


def with_polls(events, seat, n):
    """The recording in setup.js's order for `seat` (init, `n` refused readiness polls, its wallet event)."""
    out = list(events)
    wallet = out.pop(next(i for i, e in enumerate(out) if e["kind"] == "wallet" and e["seat"] == seat))
    init = next(i for i, e in enumerate(out) if e["kind"] == "cli" and e["seat"] == seat and e["argv"][0] == "init")
    poll = {"kind": "http", "phase": "setup", "seat": seat, "method": "GET", "path": "/isAuthenticated",
            "request": None, "status": None, "response": None, "error": "fetch failed"}
    out[init + 1 : init + 1] = [dict(poll) for _ in range(n)] + [wallet]
    return [{**e, "seq": i} for i, e in enumerate(out)]


class ReadinessPollsOnly(unittest.TestCase):
    """Finding 8: only the polls before a wallet's start go; an unanswered call elsewhere stays."""

    def test_floor_poll_counts_differ(self):
        lines, count = compare(with_polls(recording(), "A", 1), with_polls(recording(), "A", 4))
        self.assertEqual(count, 0, lines)

    def test_planted_a_wallet_that_stops_answering_mid_run_is_caught(self):
        a, b = recording(), recording()
        dead = {"kind": "http", "seq": len(b), "phase": SCENARIOS[3], "seat": "A", "method": "GET",
                "path": "/isAuthenticated", "request": None, "status": None, "response": None, "error": "fetch failed"}
        b.append(dead)
        code, out = run_compare(a, b)
        self.assertNotEqual(code, 0, out)
        self.assertIn("GET /isAuthenticated (seat A): only the head made this call", out)


class StubCounts(unittest.TestCase):
    """Finding 9: the stub's per-provider counts are in the recording, so a ladder change shows."""

    def with_stub(self, calls):
        r = recording()
        r.append({"kind": "stub", "seq": len(r), "phase": "sweep", "calls": calls})
        return r

    def test_floor_same_counts(self):
        calls = {"woc": {"GET /tx/{id}/hex 200": 2}, "bitails": {"GET /tx/{id} 200": 1}}
        lines, count = compare(self.with_stub(calls), self.with_stub(json.loads(json.dumps(calls))))
        self.assertEqual(count, 0, lines)

    def test_planted_the_ladder_asks_another_rung(self):
        lines, count = compare(self.with_stub({"woc": {"GET /tx/{id}/hex 200": 2}}),
                               self.with_stub({"woc": {"GET /tx/{id}/hex 200": 1}, "bananablocks": {"GET /tx/{id}/hex 200": 1}}))
        self.assertEqual(count, 1, lines)
        self.assertIn("the stub's calls at the phase's end", lines[1])
        self.assertIn(".calls.woc", lines[1])


def plant_sighash(events, old=0x41, new=0xC1):
    """The events with every input signature's sighash byte `old` rewritten `new`, as a
    build signing with the wrong byte would: each transaction's txid recomputed and
    every reference to it (a child's input, a txid field, a merkle leaf, either byte
    order) rewritten to the new one, so only the sighash byte tells the two apart."""
    raws = {}  # old txid (display hex) -> old raw tx

    def walk(v, fn):
        if isinstance(v, dict):
            return {k: walk(x, fn) for k, x in v.items()}
        if isinstance(v, list):
            return [walk(x, fn) for x in v]
        return fn(v) if isinstance(v, str) else v

    def gather(v):
        for m in D.HEX_RE.finditer(v):
            h = m.group(0)[: len(m.group(0)) // 2 * 2]
            c = D.parse_container(bytes.fromhex(h))
            if c is None:
                continue
            raw = bytes.fromhex(h)
            for t in [c["tx"]] if c["form"] == "raw" else [t["tx"] for t in c["txs"] if "tx" in t]:
                raws[t["txid"].hex()] = raw[t["span"][0] : t["span"][1]]
        return v

    walk(events, gather)
    planted = 0
    new_raw, new_id = {}, {}

    def replant(txid):
        nonlocal planted
        if txid in new_id:
            return new_id[txid]
        raw = bytearray(raws[txid])
        tx, _ = D._parse_tx(bytes(raw), 0)
        for x in tx["inputs"]:
            prev = x["prev"].hex()
            if prev in raws:
                raw[x["prev_at"] : x["prev_at"] + 32] = bytes.fromhex(replant(prev))[::-1]
            pushes = D._pushes(x["script"])
            if pushes and D._sig(pushes[0][1]) and pushes[0][1][-1] == old:
                raw[x["at"] + len(pushes[0][0]) + len(pushes[0][1]) - 1] = new
                planted += 1
        new_raw[txid] = bytes(raw).hex()
        new_id[txid] = D._sha256d(bytes(raw))[::-1].hex()
        return new_id[txid]

    for t in list(raws):
        replant(t)
    swaps = {raws[t].hex(): new_raw[t] for t in raws}
    for t in raws:
        swaps[t] = new_id[t]
        swaps[bytes.fromhex(t)[::-1].hex()] = bytes.fromhex(new_id[t])[::-1].hex()
    pattern = D.re.compile("|".join(D.re.escape(k) for k in sorted(swaps, key=len, reverse=True)))
    return walk(events, lambda v: pattern.sub(lambda m: swaps[m.group(0)], v)), planted


@unittest.skipUnless(os.environ.get("DIFFERENTIAL_RECORDING"), "DIFFERENTIAL_RECORDING=<a recording of the suite>")
class PlantedOnARealRecording(unittest.TestCase):
    """Finding 2 on the suite's own transactions: a recording post-processed, its scriptSigs' sighash 0x41 to 0xC1."""

    def test_sighash_c1_on_the_suite(self):
        a = D.load(os.environ["DIFFERENTIAL_RECORDING"])
        b, planted = plant_sighash(a)
        self.assertGreater(planted, 0)
        lines, count, *_ = D.compare_events(a, b)
        print(f"\n  planted {planted} signatures; {count} differences; earliest: {lines[1].strip()}", file=sys.stderr)
        self.assertGreater(count, 0)
        self.assertTrue(any(".sighash" in line for line in lines))


if __name__ == "__main__":
    unittest.main()
