#!/usr/bin/env python3
"""Aggregate the bounded forms of several readers or reviewers (#390, docs/lanes/ENSEMBLE.md).

    tools/lanes/aggregate.py [--options A|B|C] FILE...   one reader's or reviewer's output per file

From each file the LAST bounded block is read: an optional `VERDICT merge|fix`
line, then `DECISION:` (one line), `CONFIDENCE:` (a number in [0,1]), `RISKS:`
(`none`, or one risk per line, each ending in its own score in [0,1]) and
`RATIONALE:` (at most five lines, ended by a blank line or the end of the file).
The block starts at the last `DECISION:` line; the VERDICT, when present, is the
nearest non-blank line before it. A key may carry a list marker or bold
(`- **DECISION:**`), as a Markdown REPORT writes it.

A decision is counted by its key. With --options (trigger D's categorical
decisions: two to four option tokens joined by `|`, e.g. `A|B|C` or `yes|no`)
the key is the DECISION line's first word, which must be exactly one of the
options (trailing `,.;:` aside); a DECISION beginning with none is malformed.
Without --options the key is the VERDICT word when present, else the DECISION
line lowercased with its whitespace collapsed. The modal decision is the key
most files gave (a tie goes to the key seen earliest). The tool prints one line
per file, each risk with its score, and the summary

    AGGREGATE: consensus|split agreement=k/n mean=0.xx decision: <modal key>

A consensus is n >= 2, k/n >= 2/3 and a mean confidence >= 0.7, both compared
exactly (the confidences are read as decimal fractions; the mean is rounded to
two decimals only when printed); one file alone is never a consensus. Exit 0 is
a consensus, 1 a split, 2 a malformed block, an unreadable file (named with its
missing or bad field, nothing aggregated) or a malformed --options. A Python
script with the standard library only, like tools/scoreboard; it reads files
and runs nothing.
"""

import re
import sys
from fractions import Fraction

KEY = re.compile(r"^\s*(?:[-*]\s+)?\**(DECISION|CONFIDENCE|RISKS|RATIONALE)\**\s*:\s*\**\s*(.*?)\s*$")
VERDICT = re.compile(r"^\s*(?:[-*]\s+)?\**VERDICT\**\s*:?\s*\**\s*(merge|fix)\b", re.I)
NUMBER = re.compile(r"^(?:\d+(?:\.\d*)?|\.\d+)$")
SCORE = re.compile(r"\(?\s*((?:score\s*[:=]?\s*)?(\d+(?:\.\d*)?|\.\d+))\s*\)?\s*$", re.I)
FIELDS = ("DECISION", "CONFIDENCE", "RISKS", "RATIONALE")
RATIONALE_LINES = 5
OPTION = re.compile(r"^[^\s|]+$")
AGREEMENT = Fraction(2, 3)
MEAN = Fraction(7, 10)


class Malformed(Exception):
    pass


def unit(text):
    """A number in [0,1] as an exact fraction, or None."""
    if not NUMBER.match(text):
        return None
    value = Fraction(text)
    return value if 0 <= value <= 1 else None


def options_of(text):
    """The option tokens of --options, or raise Malformed."""
    options = text.split("|")
    if not 2 <= len(options) <= 4 or len(set(options)) != len(options) \
            or not all(OPTION.match(o) for o in options):
        raise Malformed(f"--options must be two to four distinct tokens joined by |, got {text!r}")
    return options


def parse(path, options=None):
    try:
        with open(path, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise Malformed(f"{path}: cannot read ({error})") from error
    starts = [i for i, line in enumerate(lines) if (m := KEY.match(line)) and m[1] == "DECISION"]
    if not starts:
        raise Malformed(f"{path}: no bounded block (missing DECISION)")
    start = starts[-1]

    verdict = None
    for line in reversed(lines[:start]):
        if line.strip():
            if m := VERDICT.match(line):
                verdict = m[1].lower()
            break

    found, current = {}, None
    for line in lines[start:]:
        m = KEY.match(line)
        if m and m[1] not in found:
            current = m[1]
            found[current] = [m[2]] if m[2] else []
            continue
        if current == "RATIONALE" and not line.strip():
            break
        if current and line.strip():
            found[current].append(line.strip())

    for field in FIELDS:
        if field not in found:
            raise Malformed(f"{path}: missing {field}")
    decision = " ".join(found["DECISION"]).strip()
    if not decision or len(found["DECISION"]) != 1:
        raise Malformed(f"{path}: DECISION must be one non-empty line")
    if len(found["CONFIDENCE"]) != 1 or (confidence := unit(found["CONFIDENCE"][0])) is None:
        shown = " ".join(found["CONFIDENCE"]) or "empty"
        raise Malformed(f"{path}: CONFIDENCE must be a number in [0,1], got {shown}")
    risks = []
    entries = found["RISKS"]
    if not entries:
        raise Malformed(f"{path}: RISKS is empty; write RISKS: none")
    if not (len(entries) == 1 and entries[0].lower().rstrip(".") == "none"):
        for entry in entries:
            entry = re.sub(r"^[-*]\s+", "", entry)
            m = SCORE.search(entry)
            score = unit(m[2]) if m else None
            text = entry[:m.start()].rstrip(" :-=,") if m else entry
            if score is None or not text:
                raise Malformed(f"{path}: RISKS line without its own score in [0,1]: {entry}")
            risks.append((score, text))
    if not found["RATIONALE"] or len(found["RATIONALE"]) > RATIONALE_LINES:
        raise Malformed(f"{path}: RATIONALE must be one to {RATIONALE_LINES} lines")
    if options:
        first = decision.split()[0].rstrip(",.;:")
        if first not in options:
            raise Malformed(f"{path}: DECISION must begin with exactly one of {'|'.join(options)}, got {decision}")
        key = first
    else:
        key = verdict or re.sub(r"\s+", " ", decision.lower())
    return {"path": path, "verdict": verdict, "decision": decision, "key": key,
            "confidence": confidence, "risks": risks}


def aggregate(blocks):
    counts = {}
    for block in blocks:
        counts[block["key"]] = counts.get(block["key"], 0) + 1
    modal = max(counts, key=lambda k: counts[k])  # max keeps the earliest of a tie
    k, n = counts[modal], len(blocks)
    mean = sum(b["confidence"] for b in blocks) / n
    consensus = n >= 2 and Fraction(k, n) >= AGREEMENT and mean >= MEAN
    return modal, k, n, mean, consensus


def main(argv):
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0 if argv else 2
    try:
        options = None
        if argv[0] == "--options" or argv[0].startswith("--options="):
            value, argv = (argv[1], argv[2:]) if argv[0] == "--options" and len(argv) > 1 \
                else (argv[0].partition("=")[2], argv[1:])
            options = options_of(value)
        if not argv:
            raise Malformed("no files")
        blocks = [parse(path, options) for path in argv]
    except Malformed as error:
        print(f"aggregate: {error}", file=sys.stderr)
        return 2
    modal, k, n, mean, consensus = aggregate(blocks)
    for block in blocks:
        verdict = f"VERDICT {block['verdict']} " if block["verdict"] else ""
        print(f"{block['path']}: {verdict}CONFIDENCE {float(block['confidence']):.2f} DECISION: {block['decision']}")
    for block in blocks:
        for score, text in block["risks"]:
            print(f"risk {float(score):.2f} {text} ({block['path']})")
    word = "consensus" if consensus else "split"
    print(f"AGGREGATE: {word} agreement={k}/{n} mean={float(mean):.2f} decision: {modal}")
    return 0 if consensus else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
