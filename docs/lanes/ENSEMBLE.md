# The ensemble rule (#390)

For a decision, a change, a review or a coordination point where a single wrong answer is costly, the captain asks several independent readers with distinct stances and different evidence, in isolated runs, each answering in a bounded form, and aggregates their answers before acting. Mechanical work stays with a single competent lane. The owner ruled the shape on 2026-11-01, verbatim: "I like your recommendation as long as it is efficient and productive and keeps us on god tier north star path."

This page is the rule. A cold reviewer checks a captain against it: the trigger named, the readers' shape, the forms, the aggregation, the record.

## The triggers

- **C, every lane and every review.** Every lane REPORT and every review verdict ends in the bounded form below. A review's form opens with its `VERDICT merge|fix` line. Rule 7 of [the lane rules](README.md) carries it.
- **D, a divergence or ruling call.** A result row refusing a recorded answer; a served class that moved; a fixture's meaning under the reference; a floor or gate rule. It takes three short read-only readers on DIFFERENT EVIDENCE (the BRC text at the pinned commit; the reference implementation; the package's code) with distinct stances (cautious, adversarial, simplicity), each in its own isolated run, aggregated by the captain and recorded with the disagreement shown. D's decisions are categorical, never free text: the question file's first line names its allowed answers (`OPTIONS: A|B|C`, two to four option tokens, e.g. `A|B|C` or `yes|no`), every reader's `DECISION:` line begins with exactly one of them, and the aggregator compares that token; the rationale carries the wording.
- **B, a pin PR's cold review.** Two reviewers with distinct stances: a package-reader (pins, shas, normalizers, parsers, the build and the suite) and a wire-and-reference reader (the fixtures' meaning under BRC-100 and the reference, the result rows, the classes as a MetaNet client sees them, the replays). A third, adversarial reviewer runs on a split OR on a low mean (two agreeing below a mean of 0.7); the chain prints which.
- **A, only where a production spend or a stable cut is at stake.** A full option-set ensemble before the captain's recommendation to the owner.

## The readers' shape

- **Read-only.** No reader edits anything or moves sats. A D reader reads and builds nothing; under B only the package-reader builds and runs the hermetic suite, and the wire-and-reference reader reproduces the replays and the rows (the cost rule below).
- **Short.** A reader answers the question asked, in the bounded form, and stops.
- **Different evidence.** Each reader holds one evidence pointer: under D, the BRC text at the pinned commit, the reference implementation, or the package's code; under B, the package side or the wire side of the PR.
- **Distinct stances.** Each reader's stance is named in its preamble under [stances/](../../tools/lanes/stances/): for D, `reader-brc.txt` (cautious), `reader-reference.txt` (adversarial) and `reader-package.txt` (simplicity); for B, `review-package.txt` and `review-wire-reference.txt`, with `review-adversarial.txt` on a split or a low mean.
- **Isolated.** Each reader is its own `cca run`, holding only the question and its own evidence pointer: never another reader's output, never the captain's leaning. The one exception is the adversarial reviewer under B, which forms its own verdict before it reads the other two, and says so.

Readers are never interchangeable clones. Two readers on the same evidence with the same stance count as one.

## The bounded form

After the STATUS line and the proofs (for a lane), or at the end of its reading (for a reader), every answer ends with:

```
VERDICT merge|fix          (a review only, opening the form)
DECISION: <one line>
CONFIDENCE: <a number in [0,1]>
RISKS: <a residual risk> <its score in [0,1]>
<one more risk per line, each with its own score>
RATIONALE: <at most five lines>
```

`RISKS: none` is allowed. Under D the `DECISION:` line begins with exactly one of the question's options. [aggregate.py](../../tools/lanes/aggregate.py) reads the last such block of each answer and refuses (exit 2) a block missing `DECISION` or `CONFIDENCE`, a confidence outside [0,1], or (with `--options`) a `DECISION` beginning with none of the named options, naming the file and the field.

## The aggregation

- **The modal decision** is the decision the most readers gave: under D the option token the `DECISION` line begins with, compared without case and counted as the question spells it; else a review's `VERDICT` word when present, else the `DECISION` line lowercased with its whitespace collapsed.
- **The agreement fraction** is the readers on the modal decision over all readers, written `agreement=k/n`.
- **The mean confidence** is the exact mean of every reader's `CONFIDENCE`, compared unrounded and written to two decimals only in the record.
- **A consensus** is an agreement fraction of at least two thirds AND a mean confidence at or above 0.7 on the EXACT mean, with at least two readers; one reader is never a consensus. So 2 of 2 needs the mean too (two reviewers agreeing under B at a mean below 0.7 is not a consensus: the third reviewer runs); 3 of 4 is a consensus at the mean, 3 of 5 is not, 4 of 5 is. On a consensus the captain may act within its standing words.
- **A split** is anything else. Below the fraction or the mean, the captain adds a reader with a new stance or new evidence and aggregates again, or escalates through the keeper with the disagreement stated.
- **Never a silent side.** The captain never picks one reader's answer over the others without the record below.

The tools: [chain-ensemble.sh](../../tools/lanes/chain-ensemble.sh) runs a lane, then B's two reviews concurrently from the captain's shell, the third only on a split or a low mean (it prints which), and prints one `ENSEMBLE:` line; [ensemble-read.sh](../../tools/lanes/ensemble-read.sh) runs D's three readers concurrently on a question file (its first line `OPTIONS: A|B|C`) and three evidence pointers, and aggregates with `--options`. Both take the runner from `$CCA` (default `cca`) and never merge anything. Their dry runs are CI's: the ratchet job runs [test_chain_ensemble.sh](../../tools/test_chain_ensemble.sh) under `zsh -f` with [stub-runner.sh](../../tools/lanes/stub-runner.sh) in place of `cca` and `gh`, and a failed case fails the job.

## The cost rule

The owner's condition is efficiency, and the cost here is signal and time: a D reader is a read, never a build, and short, because a build adds nothing to a reading and a slow reader delays the call; in B only the package-reader runs the hermetic suite, because a second run of the same suite yields the same bits and no new information, and the wire-and-reference reader reproduces the replays and the rows; when agreement is weak the captain adds readers with a new stance or new evidence before escalating. A is rare by definition.

## The honest limits

- The pool is one model family since the Codex cancellation, so readers' errors correlate; three agreeing readers are less independent than three people.
- A confidence is an ordering signal, not a probability; the 0.7 line ranks answers, it does not measure a chance of being right.
- The diversity that buys reliability is diversity of evidence and stance, never headcount.

## The record

The captain writes the aggregation on the PR or the issue the call belongs to, in one comment holding:

- each reader's stance, decision and confidence;
- the agreement fraction and the mean confidence;
- consensus or split;
- the action taken: acted within a standing word, added a reader, or sent to the owner through the keeper.

For example:

```
ENSEMBLE (B) on PR 412
  review-package          VERDICT merge  0.85
  review-wire-reference   VERDICT fix    0.75  fixture 096 moved under the pin
  review-adversarial      VERDICT merge  0.70
agreement=2/3 mean=0.77 consensus: merge; landed within the standing word
```

A disagreement is shown, never summarized away. Without that comment the call was not made under this rule.
