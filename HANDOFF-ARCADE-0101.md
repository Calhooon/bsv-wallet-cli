# HANDOFF: Arcade v0.10.1 consumption (issue Calhooon/bsv-wallet-cli#7)

> **STATUS 2026-07-22: IMPLEMENTED + LIVE-PROVEN** (see issue #7 comments +
> `~/bsv/epoch/arcade-v0101-campaign.md` addendum 2). Remaining here:
> (1) publish bsv-wallet-toolbox-rs 0.3.49, bump the pin, REMOVE the
> `[patch.crates-io]` dev override at the bottom of Cargo.toml; (2) redeploy
> harness daemons at a natural window WITH `CHAINTRACKS_URL` set (arms the
> FallbackChainTracker → WoC SPV gate — currently unset in prod!). Nothing
> below needs re-implementing.

**Session prompt:** "Read HANDOFF-ARCADE-0101.md and
https://github.com/Calhooon/bsv-wallet-cli/issues/7, then implement it.
Fixture-first; no daemon restarts; never lose sats."

## Context (2 min)
Arcade v0.10.1-alpha.1 (live on arcade-v2-us-1 + eu-1) delivers `merklePath` on
MINED **webhook callbacks and SSE frames** (upstream bsv-blockchain/arcade#259)
and ships verdicts-vs-conditions + ARC 460–476 codes (#260: non-final 400 →
`status: 476` retryable, NOTHING persisted server-side, fresh GET stays 404).
Proven live 2026-07-22, probe tx `104be47e…9d01` block 959011: SSE MINED event
19:06:51.907 → webhook w/ proof ingested 19:06:52.060 (arc_ingest's first-ever
real delivery; receiver edge was Cloudflare Bot-Fight-Mode-blocked until then).
Full evidence: `~/bsv/epoch/arcade-v0101-campaign.md`.

## Targets (exact)
1. **SSE-inline proof** — `~/bsv/bsv-wallet-toolbox-rs/src/monitor/tasks/arcade_events.rs`
   - Module doc line ~14 ("SSE data frames do NOT carry the merkle path") is FALSE now — fix the comment.
   - `apply_status_event` (~line 105): MINED arm currently only triggers a services fetch
     (~line 115-118). Extend the SSE event struct (`ArcadeStatusEvent`, from
     `services/providers/arcade.rs`) with optional `merklePath`/`blockHash`/`blockHeight`
     (serde default — frames without them must parse unchanged), and when present, route
     through the SAME ingest path arc_ingest uses (SPV-verify vs own chaintracks headers
     BEFORE latch). Fetch-through-services stays as the else-branch.
2. **476 → nonfinal** — status classification where submit responses / callback statuses
   are mapped (toolbox `services/providers/arcade.rs` + wallet-cli
   `src/arc_ingest.rs`). Parse additive `status` field: 476 → existing `nonfinal`
   ProvenTxReqStatus (resubmit-after-height), 466/467 → terminal buckets. Absent
   field → current behavior. A fresh GET 404 after a 476 means "no verdict — resubmit
   when final", never "abandon".
3. **Demote check_for_proofs** — `~/bsv/bsv-wallet-toolbox-rs/src/monitor/tasks/check_for_proofs.rs`:
   keep new-header trigger + 2h fallback as backstop; add a "backstop_found" counter/log —
   sustained nonzero = arcade push regression signal.
4. **Fixture tests**: enriched SSE MINED frame; enriched webhook body into
   `src/arc_ingest.rs` (tests exist: `tests/arc_callback_tests.rs` — extend with
   merklePath-bearing body); 476 submit-response classification.

## Do NOT
- Restart the live dhouse daemon on :39847 (re-registers callback tokens; see
  `~/bsv/dhouse/.harness/run-arcade-daemon.sh`). Test via `cargo test` + fixtures;
  a live re-verify rides the NEXT natural deploy window (John's call).
- Trust push: every proof re-verifies vs own headers before latch. 2xx is never success.
- Broadcast anything above tiny-value self-pay if a live probe is needed.

## Done when
`cargo test` green in both crates; fixture proves SSE-inline latch beats the
fetch path; 476 fixture maps to nonfinal; comment corrected; issue #7 updated
with evidence and cross-linked to Calgooon/rs-stack#56.
