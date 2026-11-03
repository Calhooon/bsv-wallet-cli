#!/usr/bin/env python3
"""The docs-only exit never skips a gate's input (#509, #476).

CI's `changes` job classifies the changed paths with .github/ci/docs_only.sh;
when every path is docs, the required jobs' steps skip on `docs_only`. A
docs set must exclude every file a GATE READS, not only the files the tests
walk (the keeper's class from bsv-rs, where `docs/ratchets.json` was read as
documentation and a loosened record would have skipped the ratchet). TABLE
is every required job's gate inputs, each by its job, the steps that read it
and the tool; each row plants a loosening of its input, the path a PR
changing it would carry, and feeds it to the script's `--paths` mode:

- a non-Markdown input classifies "not docs" (docs_only=false: the full set);
- a Markdown input classifies "not docs" through DOCS_READ_BY_CODE or
  PROSE_IN_CODE_FOLDERS, or every step that reads it runs on a docs-only PR,
  which the test proves by evaluating each step's `if:` in ci.yml (and its
  job's) as a docs-only pull request would. A Markdown input whose step
  skips on docs-only fails here, and the fix is its name in DOCS_READ_BY_CODE.

Each row's job and steps must exist in ci.yml and run on a full-set pull
request, each plant is a file of the tree (or a file a PR would add), and
every required job carries a row. The git mode is driven on a
scratch repository under $TMPDIR (a push, a PR's merge commit, a merge-queue
group, a tag, no base). Run by the CI job lint: python3 tools/test_docs_only.py
(`--table` prints the table with each row's classification instead).
"""

import os
import re
import subprocess
import sys
import tempfile
import unittest
from collections import namedtuple

sys.dont_write_bytecode = True  # nothing in the tree
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
SCRIPT = os.path.join(REPO, ".github", "ci", "docs_only.sh")
CI = os.path.join(REPO, ".github", "workflows", "ci.yml")

Row = namedtuple("Row", "input job steps tool loosening plant")

TESTS = ("Build the workspace's tests", "Run the workspace's tests")
SCOREBOARD = ("scoreboard",)
RATCHET = ("No number above the base's record",)
LINKS = ("links and citations",)
CHANGELOG = ("changelog fragment",)
DENY = ("deny: check adopted template hash", "deny: cargo-deny check")
LOCKS = ("locks: one bsv-rs per committed Cargo.lock, the stack at the root's versions",)
ENSEMBLE = ("ensemble chain dry runs",)
WIRE = ("The fixtures and their manifest are present", "Replay the wire fixtures")
VECTORS = ("The vector files are present", "Replay the wallet-gate vectors against the gate")
E2E = ("Check the hermetic BEEF codecs and kept-template demand", "Run the 30 scenarios against the stub")
E2E_CHECK = E2E[:1]
E2E_BUILD = ("Build the release binaries the harness drives",)
NPM = ("Install the harness's dependency (Playwright)",)
CHROMIUM = ("Install Chromium",)
FMT_CLIPPY = ("Rustfmt", "Clippy")
BUILD = ("Build workspace (release)", "Build MCP subcrate (release)", "Check the fuzz crate (every target)")

# Every required job's gate inputs: (input, job, the steps that read it, the
# tool, the planted loosening, the path a PR planting it changes).
TABLE = [
    # lint: the ratchet
    Row("docs/ratchets.json", "lint", RATCHET + SCOREBOARD, "tools/ratchet, tools/scoreboard",
        "raise a ceiling in the record", "docs/ratchets.json"),
    Row("tools/ratchet", "lint", RATCHET + LINKS, "tools/ratchet (tools/test_ratchet.py)",
        "count fewer sites", "tools/ratchet"),
    Row("tools/ratchet.d/clippy.toml", "lint", RATCHET, "tools/ratchet",
        "drop a lint the ratchet counts", "tools/ratchet.d/clippy.toml"),
    Row("the tree (src/**.rs)", "lint", RATCHET + ("Rustfmt", "Clippy"), "tools/ratchet, rustfmt, clippy",
        "add an unwrap the ratchet counts", "src/commands/send.rs"),
    Row("clippy.toml", "lint", ("Clippy",), "clippy",
        "raise a threshold", "clippy.toml"),
    Row("benches/*.rs", "lint", FMT_CLIPPY, "cargo fmt --all, clippy --all-targets",
        "a bench clippy warns on", "benches/gate_assess.rs"),
    Row("benches/common/mod.rs", "lint", FMT_CLIPPY, "cargo fmt --all, clippy --all-targets",
        "a helper rustfmt would rewrite", "benches/common/mod.rs"),
    Row("tests/**/*.rs", "lint", FMT_CLIPPY, "cargo fmt --all, clippy --all-targets",
        "a test helper clippy warns on", "tests/integration/support.rs"),
    Row("mcp/src/**", "lint", FMT_CLIPPY, "cargo fmt --all, clippy --workspace",
        "a companion clippy warns on", "mcp/src/config.rs"),
    # lint: the scoreboard and the tools tests beside it
    Row("docs/scoreboard/mutants.json", "lint", SCOREBOARD, "tools/scoreboard, tools/test_mutants_score.py",
        "lower a file's floor", "docs/scoreboard/mutants.json"),
    Row("docs/scoreboard/fuzz.json", "lint", SCOREBOARD, "tools/scoreboard",
        "drop a target's hours", "docs/scoreboard/fuzz.json"),
    Row("docs/scoreboard/equivalents.json", "lint", SCOREBOARD, "tools/test_mutants_score.py",
        "declare a caught mutant equivalent", "docs/scoreboard/equivalents.json"),
    Row("docs/scoreboard/differential.json", "lint", SCOREBOARD, "tools/scoreboard",
        "drop a scenario from the record", "docs/scoreboard/differential.json"),
    Row("docs/NORTH-STAR.md", "lint", SCOREBOARD + LINKS, "tools/scoreboard --check, tools/check-links",
        "a table cell above the record", "docs/NORTH-STAR.md"),
    Row("fuzz/Cargo.toml", "lint", SCOREBOARD, "tools/scoreboard (the fuzz targets)",
        "drop a target the record names", "fuzz/Cargo.toml"),
    Row("benches/baseline.json", "lint", SCOREBOARD, "tools/scoreboard (tools/bench-compare in bench.yml)",
        "widen a bench's tolerance", "benches/baseline.json"),
    Row(".github/workflows/nightly.yml", "lint", SCOREBOARD,
        "tools/scoreboard, tools/test_scoreboard.py, tools/test_bench_compare.py",
        "drop a Mutants shard", ".github/workflows/nightly.yml"),
    Row(".github/workflows/bench.yml", "lint", SCOREBOARD, "tools/test_bench_compare.py",
        "measure without the gate", ".github/workflows/bench.yml"),
    Row(".github/nightly/no-floored-targets", "lint", SCOREBOARD, ".github/nightly/no_floored.py",
        "leave a target out that runs a floored line", ".github/nightly/no-floored-targets"),
    Row(".github/nightly/no-floored-reach.json", "lint", SCOREBOARD,
        ".github/nightly/no_floored.py, .github/ci/affected.py",
        "zero a target's measured reach", ".github/nightly/no-floored-reach.json"),
    Row(".github/nightly/*.py", "lint", SCOREBOARD, "tools/test_changed_set.py, tools/test_mutants_score.py",
        "score a missed mutant as caught", ".github/nightly/mutants_score.py"),
    Row(".github/ci/affected.py", "lint", SCOREBOARD, "tools/test_affected.py (and the changes job)",
        "select no target for a source change", ".github/ci/affected.py"),
    Row(".github/ci/docs_only.sh", "lint", SCOREBOARD, "tools/test_docs_only.py (and the changes job)",
        "classify a record as docs", ".github/ci/docs_only.sh"),
    Row("tools/scoreboard and the tools tests", "lint", SCOREBOARD, "python3 -B tools/test_*.py",
        "let a floor fall", "tools/scoreboard"),
    Row("tools/bench-compare", "lint", SCOREBOARD, "tools/test_bench_compare.py",
        "pass a bench past its tolerance", "tools/bench-compare"),
    Row("tools/fuzz-record", "lint", SCOREBOARD, "tools/test_fuzz_record.py",
        "record a run that found a crash", "tools/fuzz-record"),
    Row("tools/testdata/**", "lint", SCOREBOARD, "tools/test_fuzz_record.py",
        "a captured run that passes a crash", "tools/testdata/fuzz-record/41413946663/artifacts.json"),
    Row("tests/wire/MANIFEST.json", "lint", SCOREBOARD, "tools/scoreboard",
        "drop a fixture from the count", "tests/wire/MANIFEST.json"),
    Row("tests/wire_consumers/**", "lint", SCOREBOARD, "tools/scoreboard (the consumers' count)",
        "drop a consumer's call", "tests/wire_consumers/app-payers/02-agents.abortAction.json"),
    Row("tests/e2e/scenarios, run.js, lib/*.js", "lint", SCOREBOARD, "tools/scoreboard (the scenario count, the knobs)",
        "drop a scenario", "tests/e2e/scenarios/planted.js"),
    Row("tests/arch_*.rs", "lint", SCOREBOARD, "tools/scoreboard (the rule count)",
        "remove an architecture rule", "tests/arch_config_env.rs"),
    # lint: links, citations and the ensemble
    Row("docs/**/*.md", "lint", LINKS, "tools/check-links",
        "a link to a path that does not resolve", "docs/lanes/planted.md"),
    Row("docs/ARCHITECTURE.md", "lint", LINKS, "tools/check-cites",
        "a citation off its pin", "docs/ARCHITECTURE.md"),
    Row("tools/check-links, tools/check-cites", "lint", LINKS, "tools/test_check_links.py, tools/test_check_cites.py",
        "pass an unresolved link", "tools/check-links"),
    Row("tools/lanes/**", "lint", ENSEMBLE + LINKS, "tools/test_chain_ensemble.sh, tools/test_aggregate.py",
        "a consensus below two thirds", "tools/lanes/aggregate.py"),
    Row("tools/lanes/stances/*.txt", "lint", ENSEMBLE, "tools/lanes/ensemble-read.sh",
        "a stance without its bounded form", "tools/lanes/stances/review-package.txt"),
    Row("tools/test_chain_ensemble.sh", "lint", ENSEMBLE, "zsh -f",
        "drop a planted case", "tools/test_chain_ensemble.sh"),
    # lint: the changelog
    Row("CHANGELOG.md", "lint", CHANGELOG, "tools/changelog",
        "a fragment's words already released", "CHANGELOG.md"),
    Row("changelog.d/<n>.md", "lint", CHANGELOG, "tools/changelog",
        "a malformed fragment (a heading)", "changelog.d/509.md"),
    Row("changelog.d/README.md", "lint", CHANGELOG, "tools/changelog",
        "remove the directory's README", "changelog.d/README.md"),
    Row("tools/changelog", "lint", CHANGELOG, "tools/test_changelog.py",
        "accept a code PR without a fragment", "tools/changelog"),
    # lint: deny and locks
    Row("deny.toml", "lint", DENY, "cargo-deny, the template hash",
        "ignore an advisory", "deny.toml"),
    Row("template/stack.toml", "lint", DENY, "the template hash",
        "stamp a loosened deny.toml's sha256", "template/stack.toml"),
    Row("Cargo.toml", "lint", DENY + ("Clippy",), "cargo-deny, clippy",
        "a git dependency", "Cargo.toml"),
    Row("mcp/Cargo.toml", "lint", DENY + ("Clippy",), "cargo-deny, clippy",
        "a crate of a banned licence", "mcp/Cargo.toml"),
    Row("Cargo.lock", "lint", DENY + LOCKS, "cargo-deny, the locks step",
        "a yanked or second bsv-rs", "Cargo.lock"),
    Row("fuzz/Cargo.lock", "lint", LOCKS, "the locks step",
        "a stack crate off the root's version", "fuzz/Cargo.lock"),
    Row(".github/workflows/ci.yml", "lint", ("the docs set: the Markdown the code reads",) + SCOREBOARD,
        "DOCS_READ_BY_CODE, tools/test_affected.py, tools/test_docs_only.py",
        "a Markdown read dropped from DOCS_READ_BY_CODE", ".github/workflows/ci.yml"),
    Row("**/*.rs, **/*.js (the Markdown the code reads)", "lint", ("the docs set: the Markdown the code reads",),
        "git grep for include_str!/read_to_string/readFile", "read a new .md", "tests/brc100.rs"),
    # build
    Row("Cargo.toml, Cargo.lock (the workspace)", "build", BUILD, "cargo build --locked",
        "a dependency that does not build", "Cargo.lock"),
    Row("mcp/src/**", "build", BUILD[1:2], "cargo build -p bsv-wallet-mcp",
        "a companion that does not build", "mcp/src/main.rs"),
    Row("fuzz/Cargo.toml, fuzz/fuzz_targets/*.rs", "build", BUILD[2:], "cargo check --manifest-path fuzz/Cargo.toml",
        "a target that does not check", "fuzz/Cargo.toml"),
    Row("fuzz/src/**, fuzz/examples/*.rs", "build", BUILD[2:],
        "cargo check --manifest-path fuzz/Cargo.toml --all-targets",
        "a property that does not check", "fuzz/src/props/beef_reader.rs"),
    Row("fuzz/Cargo.lock", "build", BUILD[2:], "cargo check --locked",
        "a lock the fuzz crate cannot build from", "fuzz/Cargo.lock"),
    Row("mcp/Cargo.toml", "build", BUILD[1:2], "cargo build -p bsv-wallet-mcp",
        "a companion manifest that does not build", "mcp/Cargo.toml"),
    # test
    Row("src/**, tests/*.rs", "test", TESTS, "cargo nextest",
        "a test that no longer asserts", "tests/wire.rs"),
    Row("mcp/src/tests.rs", "test", TESTS, "cargo nextest --workspace",
        "a companion test that no longer asserts", "mcp/src/tests.rs"),
    Row("src/ui/index.html", "test", TESTS, "src/commands/ui.rs (include_str!)",
        "the inspector page the binary embeds", "src/ui/index.html"),
    Row("tests/wire/*.json", "test", TESTS, "tests/wire.rs, tests/brc100.rs (the manifest)",
        "a fixture's response loosened", "tests/wire/037-createAction.insufficientFunds.json"),
    Row("tests/vectors/*", "test", TESTS, "tests/gate_vectors.rs, tests/arcade_webhook_vector_tests.rs",
        "a vector's verdict flipped", "tests/vectors/wallet_gate_vectors.json"),
    Row("GIFT.md", "test", TESTS, "src/commands/gift_chain.rs (include_str!)",
        "the gift's text the binary embeds", "GIFT.md"),
    Row("docs/BRC-100-EXTENSIONS.md", "test", TESTS, "tests/brc100.rs",
        "an extension the code does not serve", "docs/BRC-100-EXTENSIONS.md"),
    Row("docs/brc100/fields.json", "test", TESTS, "tests/brc100.rs",
        "drop a field of BRC-100", "docs/brc100/fields.json"),
    Row("tests/wire_consumers/**", "test", TESTS, "tests/wire_consumers.rs",
        "a consumer's call that no longer replays", "tests/wire_consumers/app-payers/01-agents.createAction.noSend.json"),
    Row("fuzz/corpus/**", "test", TESTS, "tests/fuzz_replay.rs",
        "drop a crashing seed", "fuzz/corpus/allow_pending/seed-field"),
    Row("a .md in tests/ (a code-scanned folder)", "test", TESTS, "a harness reading every file of its folder",
        "a fixture named .md", "tests/wire/planted.md"),
    # vectors
    Row("tests/vectors/wallet_gate_vectors.json", "vectors", VECTORS, "tests/gate_vectors.rs",
        "a vector's verdict flipped", "tests/vectors/wallet_gate_vectors.json"),
    Row("tests/vectors/wallet_gate_vectors.pin", "vectors", VECTORS, "tests/gate_vectors.rs (the copy's pin)",
        "a pin moved to a loosened copy", "tests/vectors/wallet_gate_vectors.pin"),
    Row("tests/vectors/wallet_gate_divergences.json", "vectors", VECTORS[1:], "tests/gate_vectors.rs",
        "a divergence recorded to excuse a row", "tests/vectors/wallet_gate_divergences.json"),
    Row("tests/vectors/arcade_status_verdicts.json", "vectors",
        ("The vector files are present", "Run the Arcade status vector through the webhook intake"),
        "tests/arcade_webhook_vector_tests.rs", "a status's verdict flipped",
        "tests/vectors/arcade_status_verdicts.json"),
    Row("tests/gate_vectors.rs, tests/gate_vectors_store/", "vectors", VECTORS[1:], "cargo test --test gate_vectors",
        "a row the replay no longer judges", "tests/gate_vectors_store/mod.rs"),
    Row("tests/arcade_webhook_vector_tests.rs", "vectors",
        ("Run the Arcade status vector through the webhook intake",), "cargo test --test arcade_webhook_vector_tests",
        "a vector read but not asserted", "tests/arcade_webhook_vector_tests.rs"),
    Row("src/gift/covenant.rs", "vectors", ("Run the covenant vectors",), "gift::covenant::tests",
        "a covenant test that skips its vectors", "src/gift/covenant.rs"),
    Row("src/gift/covenant_vectors.json", "vectors", ("The vector files are present", "Run the covenant vectors"),
        "gift::covenant::tests", "a covenant vector removed", "src/gift/covenant_vectors.json"),
    # wire
    Row("tests/wire/*.json", "wire", WIRE, "tests/wire.rs",
        "a fixture's response loosened", "tests/wire/037-createAction.insufficientFunds.json"),
    Row("tests/wire/MANIFEST.json", "wire", WIRE, "tests/wire_harness/manifest.rs",
        "drop a fixture from the manifest", "tests/wire/MANIFEST.json"),
    Row("tests/wire_harness/**, tests/wire_support/**", "wire", WIRE[1:], "tests/wire.rs",
        "a judge that passes any body", "tests/wire_harness/judge.rs"),
    # e2e-hermetic
    Row("tests/e2e/scenarios/*.js", "e2e-hermetic", E2E, "tests/e2e/run.js",
        "a scenario that no longer asserts", "tests/e2e/scenarios/planted.js"),
    Row("tests/e2e/run.js, lib/, stub/", "e2e-hermetic", E2E, "node",
        "the stub answers every call", "tests/e2e/stub/server.js"),
    Row("tests/e2e/package-lock.json", "e2e-hermetic", NPM + ("Cache Playwright's Chromium",) + CHROMIUM + E2E,
        "npm ci, the Chromium cache's key, install-browsers.js", "an unpinned Playwright",
        "tests/e2e/package-lock.json"),
    Row("tests/e2e/package.json", "e2e-hermetic", NPM + CHROMIUM, "npm ci, install-browsers.js (the Node range)",
        "a Node range that admits an unproven Node", "tests/e2e/package.json"),
    Row("tests/e2e/install-browsers.js, lib/browsers.js", "e2e-hermetic", CHROMIUM,
        "node tests/e2e/install-browsers.js",
        "install without the Node range's refusal", "tests/e2e/install-browsers.js"),
    Row("tests/e2e/24-busy-store.test.js", "e2e-hermetic", E2E_CHECK, "node --test",
        "a demand check that no longer asserts", "tests/e2e/24-busy-store.test.js"),
    Row("tests/e2e/stub/tx.test.js", "e2e-hermetic", E2E_CHECK, "node --test",
        "a codec check that no longer asserts", "tests/e2e/stub/tx.test.js"),
    Row("tests/wire/103-internalizeAction.badUnlock.json", "e2e-hermetic", E2E_CHECK, "tests/e2e/stub/tx.test.js",
        "the fixture the codec check parses loosened", "tests/wire/103-internalizeAction.badUnlock.json"),
    Row("tests/e2e/stub/fixtures/headers.json", "e2e-hermetic", E2E[1:], "tests/e2e/stub/chain.js",
        "a header chain the stub cannot serve", "tests/e2e/stub/fixtures/headers.json"),
    Row("src/**, mcp/src/**, Cargo.lock (the binaries driven)", "e2e-hermetic", E2E_BUILD + E2E[1:],
        "cargo build --release, run.js", "a served route that no longer answers", "src/server/mod.rs"),
]

REQUIRED = {"lint", "build", "test", "vectors", "wire", "e2e-hermetic"}


def unquote(value):
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] == '"':
        return value[1:-1].replace('\\"', '"')
    if len(value) >= 2 and value[0] == value[-1] == "'":
        return value[1:-1].replace("''", "'")
    return value


def workflow(text):
    """ci.yml's env (the top-level scalars), and its jobs: each job's `if:`
    and its steps' name, uses, id and if, in order. The file's layout is
    regular (two spaces a level); a step key at another indent is not read."""
    env, jobs, job, step, section = {}, {}, None, None, None
    for line in text.splitlines():
        if re.match(r"^\S", line):
            section = line.split(":")[0]
            continue
        if section == "env":
            m = re.match(r"^  ([A-Z_]+):\s*(.*)$", line)
            if m:
                env[m[1]] = unquote(m[2])
            continue
        if section != "jobs":
            continue
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            job = jobs[m[1]] = {"if": None, "steps": []}
            step = None
            continue
        if job is None:
            continue
        m = re.match(r"^    if:\s*(.+)$", line)
        if m:
            job["if"] = unquote(m[1])
            continue
        m = re.match(r"^      - (name|uses|id|if):\s*(.*)$", line)
        if m:
            step = {"name": None, "uses": None, "id": None, "if": None}
            job["steps"].append(step)
            step[m[1]] = unquote(m[2])
            continue
        m = re.match(r"^        (name|uses|id|if):\s*(.*)$", line)
        if m and step is not None:
            step[m[1]] = unquote(m[2])
    return env, jobs


TOKEN = re.compile(r"\s*(?:('(?:[^']|'')*')|(&&|\|\||==|!=|!|\(|\))|([A-Za-z_][A-Za-z0-9_.-]*)(\(\))?)")
FUNCTIONS = {"cancelled": False, "always": True, "failure": False, "success": True}


def evaluate(expression, lookup):
    """A GitHub Actions `if:` over the operators ci.yml uses (! && || == !=,
    parentheses, string literals, context paths, the status functions), its
    truth as the runner reads it. An unknown context path raises."""
    expression = expression.strip()
    if expression.startswith("${{") and expression.endswith("}}"):
        expression = expression[3:-2]
    tokens, pos = [], 0
    while pos < len(expression.rstrip()):
        m = TOKEN.match(expression, pos)
        if not m or m.end() == pos:
            raise ValueError(f"cannot read {expression!r} at {pos}")
        if m[1] is not None:
            tokens.append(("str", m[1][1:-1].replace("''", "'")))
        elif m[2] is not None:
            tokens.append(("op", m[2]))
        elif m[4]:
            if m[3] not in FUNCTIONS:
                raise ValueError(f"unknown function {m[3]}() in {expression!r}")
            tokens.append(("val", FUNCTIONS[m[3]]))
        else:
            tokens.append(("val", lookup(m[3])))
        pos = m.end()
    at = [0]

    def peek():
        return tokens[at[0]] if at[0] < len(tokens) else (None, None)

    def take():
        at[0] += 1
        return tokens[at[0] - 1]

    def truthy(v):
        return bool(v) if not isinstance(v, str) else v != ""

    def primary():
        kind, v = take()
        if (kind, v) == ("op", "("):
            v = either()
            if take() != ("op", ")"):
                raise ValueError(f"unbalanced {expression!r}")
            return v
        if kind in ("str", "val"):
            return v
        raise ValueError(f"unexpected {v!r} in {expression!r}")

    def comparison():
        left = primary()
        if peek() in (("op", "=="), ("op", "!=")):
            op = take()[1]
            right = primary()
            same = str(left).lower() == str(right).lower()
            return same if op == "==" else not same
        return left

    def unary():
        if peek() == ("op", "!"):
            take()
            return not truthy(unary())
        return comparison()

    def both():
        v = unary()
        while peek() == ("op", "&&"):
            take()
            right = unary()
            v = right if truthy(v) else v
        return v

    def either():
        v = both()
        while peek() == ("op", "||"):
            take()
            right = both()
            v = v if truthy(v) else right
        return v

    v = either()
    if at[0] != len(tokens):
        raise ValueError(f"trailing tokens in {expression!r}")
    return truthy(v)


def steps_run(job, outputs, event="pull_request"):
    """The names of the job's steps that run when `changes` answers
    `outputs`: each step's `if:` (absent: success()) in order, a skipped
    step's outcome `skipped`. An empty set when the job's own `if:` is false."""
    outcome = {}

    def lookup(name):
        parts = name.split(".")
        if parts[:3] == ["needs", "changes", "outputs"] and len(parts) == 4:
            return outputs.get(parts[3], "")
        if parts[0] == "steps" and len(parts) == 3 and parts[2] in ("outcome", "conclusion"):
            if parts[1] not in outcome:
                raise KeyError(f"step id {parts[1]} is not an earlier step's")
            return outcome[parts[1]]
        if parts[0] == "steps" and len(parts) == 4 and parts[2] == "outputs":
            return ""
        if name == "github.event_name":
            return event
        if name == "github.ref":
            return "refs/pull/1/merge" if event == "pull_request" else "refs/heads/beta"
        raise KeyError(f"no value for {name} in a step's if")

    if job["if"] and not evaluate(job["if"], lookup):
        return set()
    ran = set()
    for step in job["steps"]:
        runs = evaluate(step["if"] or "success()", lookup)
        if step["id"]:
            outcome[step["id"]] = "success" if runs else "skipped"
        if runs:
            ran.add(step["name"] or step["uses"])
    return ran


DOCS_ONLY = {"docs_only": "true"}
FULL_SET = {"docs_only": "false", "deny": "true", "test_run": "true", "wire": "true"}


def classify(paths, env, cwd=None, extra=None):
    """The script's answer for `paths`: (docs_only, its printed lines)."""
    with tempfile.TemporaryDirectory(prefix="docs-only-") as tmp:
        output = os.path.join(tmp, "output")
        open(output, "w").close()
        run_env = dict(os.environ, GITHUB_OUTPUT=output, RUNNER_TEMP=tmp,
                       DOCS_READ_BY_CODE=env["DOCS_READ_BY_CODE"],
                       PROSE_IN_CODE_FOLDERS=env["PROSE_IN_CODE_FOLDERS"], **(extra or {}))
        args = ["bash", SCRIPT] + ([] if extra else ["--paths"])
        result = subprocess.run(args, input="".join(p + "\n" for p in paths), text=True,
                                capture_output=True, env=run_env, cwd=cwd or REPO)
        if result.returncode != 0:
            raise AssertionError(f"docs_only.sh exited {result.returncode}: {result.stderr}")
        with open(output) as f:
            answers = dict(line.strip().split("=", 1) for line in f if "=" in line)
        return answers.get("docs_only"), result.stdout


def verdict(row, env, jobs):
    """(the classification, the result, None or the failure) for one row."""
    docs_only, out = classify([row.plant], env)
    kind = re.search(r"^  (.*): " + re.escape(row.plant) + "$", out, re.M)
    kind = kind[1] if kind else "?"
    if docs_only != "true":
        return kind, "full set", None if docs_only == "false" else f"docs_only={docs_only!r}"
    if not row.plant.endswith(".md"):
        return kind, "SKIPPED", f"{row.plant} is not Markdown and classified docs"
    ran = steps_run(jobs[row.job], DOCS_ONLY)
    skipped = [s for s in row.steps if s not in ran]
    if skipped:
        return kind, "SKIPPED", (f"{row.plant} classifies docs and {row.job}'s {skipped} skip on a docs-only "
                                 f"PR: name it in DOCS_READ_BY_CODE (ci.yml)")
    return kind, "its steps run on docs-only", None


class DocsOnlyInputs(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with open(CI) as f:
            cls.env, cls.jobs = workflow(f.read())

    def test_every_gate_input_is_planted_and_never_skipped(self):
        failures = []
        for row in TABLE:
            _, _, failure = verdict(row, self.env, self.jobs)
            if failure:
                failures.append(f"{row.input} ({row.job}, {row.tool}): {failure}")
        self.assertEqual(failures, [])

    def test_the_table_names_real_steps_that_run_on_the_full_set(self):
        for row in TABLE:
            self.assertIn(row.job, self.jobs, row)
            ran = steps_run(self.jobs[row.job], FULL_SET)
            for step in row.steps:
                self.assertIn(step, ran, f"{row.input}: {row.job} has no step {step!r} on the full set")

    def test_every_plant_is_a_tracked_path_or_a_new_one(self):
        # A plant is a file of the tree the input names, or a file a PR would
        # add (`planted.*`, a new changelog fragment), never a stand-in.
        tracked = set(subprocess.run(["git", "ls-files"], cwd=REPO, capture_output=True, text=True,
                                     check=True).stdout.splitlines())
        for row in TABLE:
            new = os.path.basename(row.plant).startswith("planted.") or re.fullmatch(r"changelog\.d/\d+\.md", row.plant)
            self.assertTrue(row.plant in tracked or new, f"{row.input}: {row.plant} is not in the tree")

    def test_every_required_job_has_a_row(self):
        self.assertEqual(set(self.jobs) - {"changes"}, REQUIRED)
        self.assertEqual({row.job for row in TABLE}, REQUIRED)

    def test_the_markdown_inputs_are_proven_both_ways(self):
        # The lists and the steps: GIFT.md and the extensions page by
        # DOCS_READ_BY_CODE; the scoreboard's, the links' and the changelog's
        # Markdown by their steps, which run on a docs-only PR.
        md = {row.plant: verdict(row, self.env, self.jobs)[:2] for row in TABLE if row.plant.endswith(".md")}
        self.assertEqual(md["GIFT.md"], ("not docs (the code reads it)", "full set"))
        self.assertEqual(md["docs/BRC-100-EXTENSIONS.md"], ("not docs (the code reads it)", "full set"))
        self.assertEqual(md["tests/wire/planted.md"], ("not docs (a code-scanned folder)", "full set"))
        for plant in ("docs/NORTH-STAR.md", "CHANGELOG.md", "changelog.d/509.md", "docs/ARCHITECTURE.md"):
            self.assertEqual(md[plant], ("docs", "its steps run on docs-only"), plant)

    def test_a_markdown_input_of_a_skipped_step_fails(self):
        # The test's own plant: Markdown read by the ratchet, the dry runs or
        # a Test step classifies docs and its step skips, and the row fails
        # naming DOCS_READ_BY_CODE; named there, the same row passes.
        for job, steps in (("lint", RATCHET), ("lint", ENSEMBLE), ("lint", ("Rustfmt",)), ("test", TESTS)):
            row = Row("docs/PLANTED.md", job, steps, "planted", "planted", "docs/PLANTED.md")
            kind, result, failure = verdict(row, self.env, self.jobs)
            self.assertEqual((kind, result), ("docs", "SKIPPED"), steps)
            self.assertIn("DOCS_READ_BY_CODE", failure)
            named = dict(self.env, DOCS_READ_BY_CODE=self.env["DOCS_READ_BY_CODE"] + " docs/PLANTED.md")
            self.assertEqual(verdict(row, named, self.jobs), ("not docs (the code reads it)", "full set", None))

    def test_the_conditions_on_a_docs_only_pr(self):
        lint = steps_run(self.jobs["lint"], DOCS_ONLY)
        for step in ("docs-only PR", "the docs set: the Markdown the code reads", "The base ref",
                     "scoreboard", "links and citations", "changelog fragment", "Keep the measured JSON"):
            self.assertIn(step, lint)
        for step in RATCHET + ENSEMBLE + ("Rustfmt", "Clippy") + LOCKS + DENY:
            self.assertNotIn(step, lint)
        for job in REQUIRED - {"lint"}:
            ran = steps_run(self.jobs[job], DOCS_ONLY)
            self.assertIn("docs-only PR", ran, job)
            self.assertEqual({s for s in ran if s != "docs-only PR" and not s.startswith("actions/")
                              and not s.startswith("Keep ")}, set(), job)

    def test_the_paths_mode(self):
        self.assertEqual(classify(["HANDOFF.md", "docs/lanes/README.md", "tests/e2e/README.md"], self.env)[0], "true")
        self.assertEqual(classify(["HANDOFF.md", "docs/ratchets.json"], self.env)[0], "false")
        docs_only, out = classify([], self.env)
        self.assertEqual((docs_only, out), ("false", "changes: docs_only=false (no changed path)\n"))
        self.assertEqual(classify(["README.md"], self.env)[1],
                         "changes: --paths, the paths of stdin\n  docs: README.md\n"
                         "changes: docs_only=true (1 changed paths)\n")


class GitMode(unittest.TestCase):
    """The step's own mode on a scratch repository: the diff of each event."""

    @classmethod
    def setUpClass(cls):
        with open(CI) as f:
            cls.env = workflow(f.read())[0]
        cls.tmp = tempfile.TemporaryDirectory(prefix="docs-only-git-")
        cls.root = cls.tmp.name

        def git(*args):
            return subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@invalid",
                                   "-c", "commit.gpgsign=false", *args], cwd=cls.root, check=True,
                                  capture_output=True, text=True).stdout.strip()

        cls.git = staticmethod(git)
        git("init", "-q", "-b", "beta")
        cls.write("README.md", "a\n")
        git("add", "-A")
        git("commit", "-qm", "base")
        cls.base = git("rev-parse", "HEAD")
        cls.write("HANDOFF.md", "a row\n")
        git("add", "-A")
        git("commit", "-qm", "docs")
        cls.docs = git("rev-parse", "HEAD")
        cls.write("docs/ratchets.json", "{}\n")
        git("add", "-A")
        git("commit", "-qm", "record")
        cls.record = git("rev-parse", "HEAD")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @classmethod
    def write(cls, name, text):
        path = os.path.join(cls.root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)

    def event(self, **extra):
        values = {"EVENT": "push", "REF": "refs/heads/beta", "SHA": "", "BEFORE": "",
                  "GROUP_BASE": "", "GROUP_HEAD": ""}
        values.update(extra)
        return classify([], self.env, cwd=self.root, extra=values)

    def test_a_push_of_docs(self):
        docs_only, out = self.event(BEFORE=self.base, SHA=self.docs)
        self.assertEqual(docs_only, "true")
        self.assertEqual(out, f"changes: push, {self.base}..{self.docs}\n  docs: HANDOFF.md\n"
                              "changes: docs_only=true (1 changed paths)\n")

    def test_a_merge_group_with_a_record(self):
        docs_only, out = self.event(EVENT="merge_group", GROUP_BASE=self.base, GROUP_HEAD=self.record)
        self.assertEqual(docs_only, "false")
        self.assertIn("  not docs: docs/ratchets.json\n", out)

    def test_a_pull_requests_merge_commit(self):
        self.git("checkout", "-q", "-b", "merge", self.base)
        self.git("merge", "-q", "--no-ff", "-m", "merge", self.docs)
        try:
            docs_only, out = self.event(EVENT="pull_request", REF="refs/pull/1/merge")
        finally:
            self.git("checkout", "-q", "beta")
        self.assertEqual(docs_only, "true")
        self.assertIn("changes: pull_request, HEAD^1..HEAD\n", out)

    def test_the_full_set_answers(self):
        self.assertEqual(self.event(REF="refs/tags/v1", SHA=self.docs)[1],
                         "changes: docs_only=false (a push of refs/tags/v1)\n")
        self.assertEqual(self.event(BEFORE="0" * 40, SHA=self.docs)[1],
                         "changes: docs_only=false (no base for push)\n")
        self.assertEqual(self.event(EVENT="workflow_call")[1],
                         "changes: docs_only=false (workflow_call: the full set)\n")
        self.assertEqual(self.event(BEFORE=self.docs, SHA=self.docs)[1],
                         "changes: docs_only=false (no changed path)\n")


def table():
    with open(CI) as f:
        env, jobs = workflow(f.read())
    print("| input | job | tool | classification | the planted path (loosening) | result |")
    print("|---|---|---|---|---|---|")
    for row in TABLE:
        kind, result, failure = verdict(row, env, jobs)
        print(f"| `{row.input}` | {row.job} | {row.tool} | {kind} | `{row.plant}` ({row.loosening}) | "
              f"{result}{' FAIL: ' + failure if failure else ''} |")


if __name__ == "__main__":
    if sys.argv[1:] == ["--table"]:
        table()
    else:
        unittest.main()
