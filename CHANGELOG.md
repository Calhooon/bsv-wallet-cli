# Changelog

Releases before 0.7.0 carry their notes in the release commit's message
(`RELEASING.md`).

## [Unreleased]

- `daemon` observes its server task as `serve` does (#120): a server that stops with an error (a bind error, the ctrl-c listener's fault, a panic) is logged at error level with its words (`daemon: the HTTP server stopped; stopping the monitor`), the monitor is stopped within a five-second grace, and the daemon returns the error ("the daemon's HTTP server stopped: ...") and exits non-zero; the other tasks end with the process. Before, the daemon discarded the server task's result and kept its monitor running with the server gone. ctrl-c is unchanged: the monitor stops, then the server, exit 0. Tests: `commands::daemon::tests::{a_server_that_fails_ends_the_daemon_with_its_error_within_the_grace, a_server_that_panics_ends_the_daemon_with_an_error, a_monitor_that_hangs_does_not_hold_the_daemon_past_the_grace, ctrl_c_stops_the_monitor_then_the_server_and_exits_cleanly, a_server_that_saw_ctrl_c_first_is_the_clean_exit}`.

## [0.18.1] - 2026-10-23
0.18.1: two pin moves, no CLI code change. `bsv-wallet-toolbox-rs` `=0.12.1` to `=0.13.0` (toolbox #44, PR 50: `is_utxo_with_spender(txid, vout, script) -> (UtxoVerdict, Option<BoundSpender { txid, confirmed }>)` beside the unchanged `is_utxo`, four requests per check) and `bsv-rs` `=0.8.1` to `=0.8.2` (bsv-rs #73, PR 74: `Error::WalletTransport { status, detail }` for a bare 502, 503 or 504 with no busy code, a connection error or a timeout, retryable and promising nothing about what the wallet did; `WalletBusy`'s reading unchanged). `bsv-tracker` `=0.8.0` unchanged. The lock moves those two entries and this crate's version only.

- The spend probe (`probe_input_spend`) stays on its own two steps for now. Moving it onto `is_utxo_with_spender` (one ladder pass, four requests per outpoint instead of four to six) was built and reviewed, and the review found two toolbox-side gaps the move would expose in the release path: an unreadable spender name falls through to the unspent hint, where the probe's own step held it as "could not look" and never released; and with no header service the ladder names no spender where the probe named the bound one unconfirmed. Filed on the toolbox; the probe moves in the patch that pins the fix.
- The CLI's handler still classifies errors by prefix (#81); the typed transport fault is what #81 keys on.

## [0.18.0] - 2026-10-23
0.18.0: the spend gate's two answers from LOW's fleet and the 0.17.1 gate (#59, #63). Pins unchanged: `bsv-wallet-toolbox-rs` `=0.12.1`, `bsv-rs` `=0.8.1`, `bsv-tracker` `=0.8.0`. A minor: a new wire shape for `allowPending`, a new flag, a new served code; no new environment.

#59 (the gate's answer on a header-service transport fault; the 0.17.1 gate's first attempt and LOW's vultr funder, bsv-low #856, 2026-10-23: a two-second blink of the header service held every coin "headers unavailable" and the caller read `402 INSUFFICIENT_FUNDS`, a balance verdict, for a transport fault).

- `tracker_host::CreatorReading` carries `headers_fault`: the fault is the header service's (`CheckError::Headers`: its tip or a header the check needs could not be read), as against a header the snapshot does not hold (`Unavailable`, a block above its tip), a path that does not parse, or a root that does not match. A `Headers` fault's text is the service's own error (the toolbox 0.12.1 ladder's "<call>: no header service answered (chaintracks: ...; bhs: ...)"), no longer the variant's Debug (`Headers(HeaderError("..."))`). `spend_gate::HeldCoin` carries the same mark.
- The gate reads once more: when a batch's creators were held on the service's fault, the gate waits `HEADER_RETRY_PAUSE` (two seconds, the blink's length) and reads those creators again, once per gated call (`Assessment::headers_retried`; the door's full read retries only if its first read did not); a named input held on the fault is re-read the same way. `assess` is unchanged (one read); `assess_retrying` is the door's. Logged at warn (`spend gate: the header service did not answer; reading it once more`, with the fault) and at info with how many answered.
- The answer: when the coins still held on the fault, with what was offered, reach the outputs' sum (the floor of what the action needs: a shortfall they could not have covered is the funds' refusal whatever the service said), or every named input is held on it, the refusal is "could not verify now" (`Refusal::headers_fault`, the service's error; `Refusal::code` `HEADERS_UNAVAILABLE`): the route answers `503 HEADERS_UNAVAILABLE` with `Retry-After: 2` and the refusal's body (`message` "Could not verify now: the header service did not answer (<error>); N coin(s), S sats, held on it, no verdict (...)", `offeredSatoshis`, `held`); the commands print the same body under `--json`, exit 1. Nothing was written, built, locked or posted. `402 INSUFFICIENT_FUNDS` and `403 INPUT_NOT_OFFERED` are unchanged where no coin is held on the service.

#63 (LOW's loop 46, pair 32: `allowPending: received` is one word for every received unproven coin, so a funder that just paid a seat could not name its own top-up alone when the seat held a payout from elsewhere).

- `spend_gate::AllowPending`: `Off`, `Received` (the string form, every received unproven coin), `ReceivedOnly(coins)` (the scope). The served `/createAction` takes `allowPending: {"received": ["txid:vout", ...]}` beside the string form (`AllowPending::from_json`: an empty list is off, an object of another shape or an outpoint that does not parse is `400 VALIDATION_ERROR`, any other JSON value is off as before); `send`, `drain`, `split` and `gift-send` take `--coin <TXID:VOUT>` (repeatable) with `--allow-pending received` (`AllowPending::from_cli`; `--coin` without the word is the caller's error, clap's `requires`). `txid.vout`, as the listings print an outpoint, is taken too; txids are lowercased, the list sorted without repeats.
- The gate (`hold_reason` now takes the coin's outpoint): under the scope only the named received coins are offered; every other received unproven coin is held `depth N: M blocks; received, no proof; not named under allowPending: received` (`NOT_NAMED_HINT`). The action carries R1 (d)'s label under the scope as under the word. `GateOptions.allow_received: bool` is `allow_pending: AllowPending` (`with_allow_received(bool)` kept; `with_allow_pending` added).
- Before the door (`check_scope`): each named coin must be a received coin this wallet could spend under the word (one of its outputs, change of the default basket, not consumed, its transaction `completed` or `unproven`, not an action of ours); the first that is not refuses the call, `GateError::Scope`, "Invalid argument: allowPending: received names <txid>:<vout>, which is not a received coin this wallet could spend under it: <why>" (`not a coin of this wallet`, `own change of an action of this wallet, not a received coin`, `spent by <txid>`, `its transaction is <status>`, `not a change coin of the default basket`); the route answers `400 VALIDATION_ERROR`, the commands print `{"code":"VALIDATION_ERROR","message"}` under `--json`, exit 1; nothing is read for the action. A named coin already mined is offered as any mined coin: the word is moot for it, not wrong.
- The MCP tool `create_action` carries the string form only (its `allowPending` is a string; unchanged, the MCP crate stays 0.3.0).
- The e2e harness: the client's thrown error carries `status` and `body` (a refusal's `code`, `held`); scenario 23 (`scoped-allow-pending`) starts a third wallet C, pays it twice from A (two received unproven coins, whatever block came during the run), and drives the scope: 402 over it with P2 held "not named" and P1 offered, 2,000 within it on P1 alone, 400 naming the spent P1 and a foreign coin, the plain word returning the rest to A; fees only. 23 scenarios.
- The e2e harness's ports and guard (the 0.18.0 gate's first run, 20261023T195137Z, 19/23): the harness's default port for wallet A, 3323, was the port `~/bsv/prod-test-wallets/REGISTRY.md` reserves for its `recipient` seat, whose daemon was serving; the harness's own `serve` died "Address already in use", its health check answered from that daemon, and the run funded, drove and swept a seat it did not own (the seat's own sats went to the e2e funder with the sweep; recorded in HANDOFF). Now: the test wallets default to 3370 (A), 3371 (B), 3372 (C), off every registry port (`WALLET_A_PORT`, `WALLET_B_PORT`, `WALLET_C_PORT` move them); `startWallet` refuses a port that already answers before anything is spawned, and after the spawn checks the server's identity against the directory's own key (`bsv-wallet identity`), refusing a stranger. The four failures of that run (2, 3, 16, 17) were the CLI reading the directory's key while the HTTP answers came from the stranger; scenario 23 passed.
- `HeldCoin.output_id` is public (the handler tests build a held coin).
- Tests: `spend_gate::tests::{the_scoped_word_offers_only_the_named_received_coins, a_scoped_word_writes_the_label_and_a_mined_named_coin_passes, a_named_coin_that_is_not_a_received_spendable_coin_is_refused_before_the_door, the_word_is_parsed_from_the_route_and_the_flags, a_header_service_fault_is_read_once_more_and_is_could_not_verify_now, a_shortfall_the_faulted_coins_could_not_cover_stays_insufficient_funds, the_re_read_offers_the_coin_when_the_service_answers_again, a_named_input_held_on_the_fault_alone_is_could_not_verify_now}`; `server::handlers::tests::{the_header_services_fault_is_503_headers_unavailable_with_retry_after, a_scope_naming_a_wrong_coin_is_the_callers_400}`.

## [0.17.2] - 2026-10-23
0.17.2: `bsv-wallet-toolbox-rs` `=0.12.0` to `=0.12.1` (bsv-wallet-toolbox-rs #47, PR 48: the header ladders answer "<call>: no header service answered (chaintracks: <error>; bhs: <error>)" when every rung fails, "no header service configured" only when neither client is configured, rung failures logged at warn; the ladders' order and request count per call unchanged). The spend gate's "headers unavailable" hold and the monitor's `new_header` warning now carry the header service's own error (the 0.17.1 gate's first attempt, CHANGELOG 0.17.1). `bsv-rs` `=0.8.1` and `bsv-tracker` `=0.8.0` unchanged; no CLI code change. CLI #59 (the gate's answer on a header-service transport fault: a retry and a 503, never 402 INSUFFICIENT_FUNDS) stays open for a later minor.

- The e2e harness's auditor (`tests/e2e/lib/woc.js` `verifyOnChain`): the explorer ladder, WhatsOnChain then Bitails then BananaBlocks on every poll, the first 200 the answer (`listedBy`), a 404 from one explorer "not yet there", a fault moved past; the throw names every rung's last answer. The 0.17.2 gate's run 20261023T165643Z: 21/22, scenario 14 failed on WoC alone answering 404 for a broadcast return payment past the 88-second poll while Bitails listed it at six seconds and the wallet's own ladder recorded it as seen; both transactions are on chain.

## [0.17.1] - 2026-10-23
0.17.1: the repair passes measure the balance and the spend gate names a consumed input as spent (#53). Pins unchanged: `bsv-wallet-toolbox-rs` `=0.12.0`, `bsv-rs` `=0.8.1`, `bsv-tracker` `=0.8.0`. A patch: no new command, flag or environment; two report lines and one listing reason reworded, their figures measured.

- The e2e harness (`tests/e2e/run.js`): a failure before the scenarios (the funder refusing the funding, a wallet that would not start) is printed as FATAL and exits 1, where it used to exit 0 as "0/0 passed" with the error unprinted; a run that ends with fewer results than it was asked for (all, or one under `--scenario N`) exits 1; a dry run or a single scenario no longer keeps the wallet directories as a failed run. Seen on the 0.17.1 gate's first attempt: the header service blinked for two seconds and the funder held every coin (bsv-wallet-toolbox-rs #47 for the misnamed reason).
- `Cargo.lock`: `tokio` 1.51.0 to 1.54.1 for RUSTSEC-2026-0386 (an unsound advisory on `tokio::io::unix::AsyncFd`'s safe constructors, published after 0.17.0; `cargo deny check` fails the graph on it under `unsound = "all"`). This crate never constructs an `AsyncFd`; the bump is the paved road's rule, not a fix to a path we run. No other package moves; the `windows-sys` edge of `errno`, `rustix` and `tempfile` re-resolves to 0.52.0, a version the lock already carried (Windows only; the paved road builds none).

#53 (LOW's fleet audit, 2026-10-23, seats p3, p7, p35: `reconcile-outputs --execute` printed "Net balance delta: -365540 sats" while `balance --json` read the same satoshis before and after).

- `reconcile-outputs` and `cleanup-abandoned`: the "Net balance delta" line, a sum over the rows the pass touched, is gone. The report measures the default basket's spendable sum before the pass and after it (`commands::balance::default_basket_sats`: `spendable = 1`, the creator `completed`, `unproven`, `nosend` or `sending`, the `default` basket not deleted, the toolbox 0.12.0 `list_outputs` filter `balance` reads) and prints "Balance (default basket): B1 -> B2 (delta)", then "Turned off: N rows, S sats in <basket>" per basket from the UPDATEs that changed a row (guarded on `spendable = 1`; on a dry run the rows that would). `cleanup-abandoned` adds "Turned on: ..." for restored inputs, reads the balance again after the descendants' retirement, and no longer counts a relinquished input that was already off as balance (a locked input reads `spendable = 0`). "Restart the daemon to refresh its in-memory view." stays. Report fields: `balance_before`, `balance_after`, `turned_off` (both), `turned_on` (`cleanup-abandoned`), `already_off_count`/`already_off_sats` and `relinked_from_chain_sats` (`reconcile-outputs`).
- `reconcile-outputs` phase (a): the re-linked sats count only rows that were `spendable = 1`; a row a live tx of ours spends that was already off is reported apart ("already off: N rows, S sats") and still gets its `spent_by` link.
- The spend gate: a coin whose `spent_by` is a live action of ours reads "spent by <txid> (unproven)", not "locked by <txid>": it is an input that action consumed, spent for good, never a hold, and it leaves the listing when the spender is completed or failed. A coin a caller names whose spender is completed reads "spent by <txid>", a failed one "spent by <txid> (failed)". The `word` stays `locked`.
- Tests: `reconcile_outputs::tests::{a_relinquished_output_in_another_basket_leaves_the_balance_where_it_was, a_phase_a_row_already_off_is_named_apart_and_still_linked, the_measured_delta_equals_the_default_basket_rows_turned_off}`; `cleanup_abandoned::tests::{a_relinquished_output_in_another_basket_leaves_the_balance_where_it_was, a_relinquished_input_already_off_is_not_turned_off, the_measured_delta_equals_the_default_basket_rows_turned_off, a_restored_input_turns_on_and_the_delta_nets_it_against_the_phantom}`; `spend_gate::tests::{a_consumed_coin_is_listed_as_spent_by_its_spender, the_spent_by_reason_names_the_spenders_state}`.

## [0.17.0] - 2026-10-22
0.17.0: BananaBlocks as a courier rung in every chain check, under the owner's rule (#50, 2026-10-22): each chain scanner has its own rate limits, so when one fails we go to the next; WhatsOnChain, BananaBlocks and Bitails are each other's fallbacks; a negative ("unspent", "absent") needs a second provider's clean answer; "could not look" is never "nothing there". Pins: `bsv-wallet-toolbox-rs` `=0.11.0` to `=0.12.0` (BananaBlocks as the third explorer in the services' ladders, the toolbox's `RateGate`, WhatsOnChain no longer sleeping on a 429; bsv-wallet-toolbox-rs PR 43); `bsv-rs` `=0.8.1` and `bsv-tracker` `=0.8.0` unchanged. A minor: new environment, a new log line, `sync`'s scan through the toolbox's lead sources.

- `src/explorers.rs`, the CLI's own rungs: WhatsOnChain (its key, a rate gate: a 429 is a fault at once and the rung is skipped without a request until Retry-After passes) and BananaBlocks (the toolbox's provider: 60 a minute, the same gate). One budget per host: the WhatsOnChain rung passes the toolbox WhatsOnChain provider's own gate (`WhatsOnChain::rate_gate`, the process's for its base and key), so a 429 either the CLI's rung or the toolbox's ladders meet closes WhatsOnChain for both until Retry-After passes; `Explorers::at` (fixtures, mirrors) keeps a gate of its own. Test: `the_whatsonchain_rung_shares_the_toolbox_providers_gate`. Every explorer call logs marker `explorer_call` at info with `provider`, `call`, `status` and `ms`, so an audit can see which rung answered.
- The spend probe (`probe_input_spend`; `cleanup-abandoned`, `reconcile-outputs`, the daemon's sweep, `sync --reconcile-spent`): step 1, "who spent it", is a ladder of WhatsOnChain's `/tx/{txid}/{vout}/spent` and BananaBlocks' `/txo/{txid}/{vout}` with a rotating start (BananaBlocks' output under another script than the probe's, the empty one included, is a fault); a fault moves on at once; a name from either is bound by the spender's own bytes and its proof as before; one explorer's "no spend known" falls through to step 2, the toolbox's `is_utxo`, now over three explorers, where "unspent" and "not listed" each need two explorers' clean answers (the toolbox's `UTXO_POSITIVE_PROVIDERS` and `UTXO_NEGATIVE_PROVIDERS`, two of the three) and one answer with the others faulting is "could not look". A name whose bytes could not be read is `Unknown` unless another rung's name binds.
- `sync`: the deposit address's scan is the toolbox's `get_utxo_leads` with the deposit script itself (`GetUtxoStatusOutputFormat::Script`), not a hand-rolled WhatsOnChain `/address/{address}/unspent`: every lead source is asked, WhatsOnChain and Bitails by the script's hash and BananaBlocks by the deposit address. A lead is discovery, not a verdict: one explorer's list is enough to look, the union of every answering explorer's list (keyed by outpoint) is the fullest look, and each lead is received only after `receive` binds its bytes to its txid and checks its proof against the header service. A WhatsOnChain fault no longer ends the sync with "WoC unspent fetch failed"; an empty set is every answering explorer's empty list; when no explorer answered the sync is an error saying "could not look". `--reconcile-spent`: an outpoint of ours missing from the leads is only a question for the spend probe, never "spent"; what the probe cannot decide is counted undecided and left alone. `--json` adds `unspent_listed_by` (the explorers that answered), `unspent_could_not_look` (those that faulted), `unspent_partial` (those whose list was cut short) and `unspent_leads` (each lead's outpoint and the explorers that listed it). A list cut short (the toolbox reads each explorer's list page by page, at most 10 pages of 1000, `LEAD_PAGES_MAX`; a list past the cap, or one whose later page could not be read, is `UtxoLeads::partial`) is still a list: its leads are received after the same bytes and proof check, and an outpoint of ours missing from it is only a question for the spend probe. The summary line adds "N explorers answered (M cut short: names)" when M > 0, and when every answering list was cut short it says the sync was not a full look: an output past the cap is reached by a later sync only if the explorers' ordering brings it into the pages read, which is not promised.
- `receive`: the courier ladder is WhatsOnChain's `/tx/{txid}/beef`, then BananaBlocks' `/tx/{txid}/hex` (bytes that hash to the txid, with the proof the wallet's services return only after its root met the header service; no checked proof is a fault, the next courier asked), then the toolbox's `get_beef`. A 429 moves on at once.
- `broadcast_verify`: BananaBlocks `/tx/{txid}` is a third chain index, read through the toolbox's provider: its `mined` is chain evidence; its `known` is presence and nothing more, read as a broadcaster's `SEEN` (as the toolbox 0.11.1 and 0.12.0 read a status source's `known`: below a checked proof, never a chain index's word), so it never ends the question (a `mined` from any index outranks it), it is credited as the network's word and never the chain's, and it does not stand against WhatsOnChain's and Bitails' clean 404s; its 404 is a gap and never an absence vote; absence stays WhatsOnChain's and Bitails' clean 404s both. Test: `a_mined_outranks_bananablocks_known_and_known_is_never_chain_evidence`; `bananablocks_mined_is_chain_evidence_named_by_its_rung`'s second half now expects the `known` as a word (`known_by`), not `Present(Seen)`. `PresenceReport::chain_index_by` names the index that answered; `reconcile-broadcasts` logs it when it credits a transaction.
- Environment (`src/services_env.rs`): `WOC_API_KEY` and `BITAILS_API_KEY` reach `ServicesOptions`; `BANANABLOCKS_BASE` (unset the chain's default, empty off, a URL moves it).
- The server's `GET /tx/{txid}` is unchanged: it answers the tracker's stored word and asks no explorer (E3), so it has no rung to name.
- `tests/rule_28_sites.rs` moves with the sites: the CLI's WhatsOnChain request line is in `src/explorers.rs`; `sync.rs`, `receive.rs`, `cleanup_abandoned.rs` and `reconcile_outputs.rs` hold no request and no client; the BananaBlocks chain index is listed in `broadcast_verify.rs`.
- Tests (no live network, a fixture per rung): `a_woc_429_and_a_bananablocks_spender_bound_by_bytes_is_spent_by`, `a_positive_from_either_rung_whichever_is_asked_first`, `one_negative_and_faults_is_could_not_decide`, `an_unreadable_name_is_unknown_unless_another_rung_binds` (the probe); `a_whatsonchain_fault_with_bitails_listing_is_received_through_bananablocks`, `whatsonchain_and_bitails_faulting_with_bananablocks_listing_completes`, `every_explorer_faulting_is_could_not_look_never_an_empty_sync`, `a_lead_from_one_explorer_is_received_only_after_its_bytes_and_proof_check`, `missing_from_one_explorers_set_is_could_not_decide_never_spent`, `a_cut_short_explorers_leads_are_received_through_the_trait`, `a_list_through_the_trait_is_received`, `unspent_partial_names_the_cut_short_explorer`, `the_summary_line_says_a_list_was_cut_short` (sync); `a_woc_429_reaches_bananablocks_at_once_and_its_bytes_carry_the_proof`, `the_bananablocks_rung_moves_on_without_bound_bytes_or_a_checked_proof` (receive); `bananablocks_mined_is_chain_evidence_named_by_its_rung`, `bananablocks_unknown_is_a_gap_and_a_negative_needs_two_clean_404s`, `the_woc_mempool_body_without_confirmations_is_seen` (overlay #606's body) (the verifier); `the_pass_hears_bananablocks_when_whatsonchain_is_rate_limited` (reconcile-broadcasts); `a_whatsonchain_429_closes_the_rung_for_its_retry_after`, `the_woc_api_key_is_sent`, `bananablocks_spend_route_is_read_through_the_toolbox`, `bananablocks_bytes_are_bound_to_the_txid` (the rungs). Moved with reasons: sync's `the_address_scan_is_named_at_the_site` is `the_address_scan_asks_the_ladder_and_is_named_at_the_site` (it now holds the site to `get_utxo_leads` with the script itself); in review, sync's `sync_with_woc_faulting_and_bitails_answering_completes` became `a_whatsonchain_fault_with_bitails_listing_is_received_through_bananablocks` and `no_explorer_giving_the_set_is_could_not_look_never_empty` became `every_explorer_faulting_is_could_not_look_never_an_empty_sync` (the scan asks for leads, not the verdict ladder; under the toolbox's partial-leads commit the mock answers an unset `get_utxo_leads` from its unspent list, an empty answer, so the test now asks through the trait for both shapes of "could not look": `NoExplorerAnswered` and an answer naming no explorer); the WoC-only probe and courier tests run on a WhatsOnChain-only ladder (`woc_only`).
- The deliberate respend under toolbox 0.12.0 (0.11.1's rules: Arcade's `known` for its SEEN words only, a failed ancestor refused before broadcast, nothing released on an explorer's word): `respend`'s code is unchanged; two witnesses state the rule as it stands. `a_phantom_permits_only_once_arcade_stops_seen`: the status ladder's `unknown` (two chain indexes' 404s standing against Arcade's `known`) is not enough while Arcade's own word is a SEEN word or a holding word (`RECEIVED`, `ACCEPTED_BY_NETWORK`); it permits once Arcade stops, and releases nothing either way. `a_single_gap_at_the_status_ladder_refuses_and_releases_nothing`: one chain index that could not look leaves the phantom out of the ladder's answer with "could not look", and the respend refuses on both of that answer's shapes. No existing respend test rested on 0.11.0's wider `known` (their mocks answer `known` only with Arcade's `Seen`), so none changed.
- The spend gate (`src/spend_gate.rs`) and the tracker host (`src/tracker_host.rs`) are untouched: they read the wallet's own proof records, never an explorer.

## [0.16.1] - 2026-10-22
0.16.1: the pin moves to bsv-rs =0.8.1 (bsv-wallet-toolbox-rs =0.11.0 and bsv-tracker =0.8.0 unchanged). No CLI code changes.

- bsv-rs 0.8.1: the atomic BEEF rule is judged on the subject's graph (BRC-95), not wire position. 0.8.0 refused an atomic BEEF whose raw unproven ancestor sat after the proven transaction that spends it, the order the TypeScript SDK 2.x writer emits: five x402 refunds were refused by a served wallet (bsv-dd P0-65). Both readers now accept every entry reachable from the subject through the inputs of raw entries and refuse nothing 0.8.0 accepted; the refusal names the earliest entry the walk from the subject does not reach. The crate's atomic writer emits the BRC-95 closure cut at proofs, as the TypeScript 3.x and Go writers do, so what this wallet hands over is accepted by every reader; a wallet's `createAction` BEEFs shrink by what no reader needs. The stream's Cursor format is BSC3; a BSC2 cursor still reads. The owner's ruling of 2026-10-22 (the stack's DECISIONS-2026-10-19.md section 10; epoch channel lines 1044 and 1046); the upstream findings filed as bsv-blockchain/BRCs#350, ts-stack#1320, go-sdk#384.
- The e2e harness since 0.16.0 (on main already): the sweep retries short by the refusal's named shortfall; the budget judges spent sats apart from stranded ones.

## [0.16.0] - 2026-10-21
0.16.0: the CLI's half of Calhooon/bsv-wallet-cli #44 beside bsv-low #828. A served `/createAction` answered `400` with `database is locked (code 517)` (`SQLITE_BUSY_SNAPSHOT`: a deferred `BEGIN` read under a WAL snapshot and wrote after the monitor's commit); and LOW's land condition, the spend gate writing nothing around the toolbox's door. Pins: `bsv-wallet-toolbox-rs` `=0.10.0` to `=0.11.0`, `bsv-rs` `=0.7.0` to `=0.8.0`, `bsv-tracker` `=0.7.0` to `=0.8.0`; the lock carries one bsv-rs, 0.8.0. A minor: the 503's rule, the 409, and `pool()` no longer writes.

- Every write on the store's writer. The toolbox opens two pools over the file: `pool()` is the reader (`query_only`), `writer()` one connection, `write_tx()` `BEGIN IMMEDIATE` on it. The CLI's ten transactions (`respend` 2, `spend_gate` 1 (`heal_holds`), `spend_guard` 2, `tracker_host` 1, `cleanup_abandoned` 4) open on `write_tx`, its single-statement writes run on a connection of `writer()` acquired through `store::writer` (a writer not acquired within the ceiling is the toolbox's typed `StoreBusy`), and every test seed that wrote through `pool()` moved. The schema helpers read first and send their DDL to the writer only when something is missing (`src/store.rs`). A chain event's header snapshot is taken before its transaction, so no write lock is held across a service await. `cleanup-abandoned` and the daemon's reconcile pass use the context's store instead of a second pool on the file. The three `PRAGMA busy_timeout = 500` sites (`tick`, the daemon tenant twice) are gone: they lowered the store's 5000 on one reader connection. `clippy.toml` disallows sqlx's `Pool::begin`, `begin_with`, `Connection::begin` and `Acquire::begin`.
- The offer replaces hold-and-restore (bsv-low #828; #44). The gate's `createAction` is one `Wallet::create_action_offering(args, originator, offer)`, the offer the coins the assessment offers; the toolbox picks and locks within it in its one transaction. No hold before the door, no restore after it. `heal_holds` makes change again every coin a 0.15 process left in `spend_gate_holds`: at `daemon` and `serve` start (a start whose heal failed owes it, and the next served request tries again), and in the gated path of `send`, `drain`, `split` and `gift-send` after a read on the reader finds a hold there (`heal_if_held`), so a user who never serves gets those coins back; nothing writes the table. When the lazy offer draws `Unspendable coin:` the gate reads every coin and runs the call once more, as it does for no funds. One offer names at most 16384 coins (`OFFER_CAP`), the largest by value: the toolbox binds every offered id in one statement, under SQLite's limit of 32766 variables; a refusal names the coins the cap left out, which are neither offered nor held back (`; N coins (S sats) beyond the offer's cap of 16384, the largest offered`). Tests: `the_offer_allocates_only_the_offered_coins_and_writes_no_hold` (red on the plain `createAction`), `the_heal_at_start_restores_the_holds_a_015_process_left`, `the_commands_heal_reads_first_and_heals_only_a_hold`, `a_served_request_retries_the_heal_a_failed_start_owed`, `an_unspendable_coin_in_the_lazy_offer_falls_through_to_every_coin`, `the_offer_is_capped_at_the_largest_coins`, `a_refusal_names_the_coins_past_the_offers_cap`, `an_offer_at_the_cap_runs_and_one_past_sqlites_limit_does_not`.
- The rule record as a label. `allowPending: received` rides as the action's label `spend-gate:received-pending`, appended before the door and written by the toolbox with the action; a deferred template keeps it through `signAction`, and `respend` labels its replacement in its own transaction. The label is the gate's alone: a caller's own copy (any case, any padding) is stripped from the gate's `createAction` and the served `internalizeAction` before the toolbox sees it, so a forged label is neither `underRule` nor a respend's `allow_received`. `actions` and `/listActions` derive `underRule` from the label first, then from a 0.15 store's `spend_gate_actions` rows (a reference row names its txid at the read). The gate writes nothing after the door: `record_rule`, `fill_txid` and the signAction handler's write are gone. The served `createAction` handler's follow-up still writes after it (the network presence and the mined evidence, the ancestors' credit, the retire of an absent broadcast); every failure there is logged with the marker `post_broadcast_follow_up_write_failed` (the txid, the error) and is never the call's error. Tests: `a_forged_rule_label_is_stripped_and_grants_nothing`, `internalize_strips_the_gate_rule_label`, `a_failed_follow_up_write_is_logged_with_its_marker`, `the_gate_writes_nothing_after_the_door` (SQLite's update and commit hooks on the writer, counted as the door returns and as the gate returns), `a_015_record_of_the_word_keeps_answering`, `a_015_template_record_answers_before_and_after_its_signing`, `a_template_under_the_word_carries_its_label_and_keeps_it_aborted`.
- The answer's rule, typed. `503 STORE_BUSY`, body `{"code":"STORE_BUSY","message"}`, `Retry-After: 1`, only for the store busy by its type: `bsv_sdk::Error::WalletBusy` from a wallet door, or a pre-door read or write of the gate or the guard whose error is SQLite's code 5 or the writer's acquire ceiling (or the toolbox's typed `StoreBusy`); a bare `PoolTimedOut` (the reader's 30 s acquire) is a plain storage fault, 500. The substring belt of e0ff758 (`is_store_busy`, the message patterns) is deleted; the words `Store busy:` are no signal. Every other fault keeps its word and status: the gate's own faults 500 `Storage error: spend gate: ...`, the toolbox's errors by their prefixes, broadcast faults as before. The toolbox's new `Unspendable coin:` is `409 UNSPENDABLE_COIN`, the coin named under `coin`; `Service error:` stays 502. CORS exposes `Retry-After`. Tests (`src/server/handlers/tests.rs`): `no_message_is_store_busy_by_its_words_and_no_4xx_says_it`, `the_typed_wallet_busy_from_the_door_is_503_retry_after_one`, `a_pre_door_fault_is_503_only_when_it_is_the_store_busy`, `the_unspendable_coin_is_409_and_names_the_coin`, `cors_exposes_retry_after`; `only_the_typed_busy_is_busy`, `sqlites_own_busy_is_busy`, `only_the_writers_acquire_timeout_is_busy` (`src/store.rs`).
- After a post, the post's truth (the other half of the 503's rule). A store fault after a transaction is posted is never the call's error; the answer carries the txid and the fault is logged with a marker. `split`: a failed self-internalize after the broadcast (`post_broadcast_split_internalize_failed`) still prints the txid, adding `"selfInternalized": false` under `--json` (stdout was empty). `respend`: the status write and the spend guard after `post_beef` (`post_broadcast_status_write_failed`, `post_broadcast_guard_failed`) and the command's read of the request (`post_broadcast_request_read_failed`, read as `unknown`) no longer lose the replacement's txid. Tests: `a_failed_self_internalize_after_the_post_still_answers_the_txid`, `a_store_fault_after_the_replacement_is_posted_is_logged_and_answers_its_txid`.
- One store per file per process. The daemon opened its wallet file twice, one `StorageSqlx` for the monitor and the chain feed and one for the served wallet, so their writes met on SQLite's busy handler between two writer pools. `store::open_shared` opens it once and answers the store for the `Wallet` and `Arc::new(store.clone())` for the `Monitor` (toolbox 0.11.0: a clone shares the reader, the one-connection writer, the reservations and the state), so every writer in the process queues on one fair pool. `reconcile-outputs` ran on its own `SqlitePool` beside its context's store; it now runs on the context store's writer (its statements are single autocommits), and the command loses its database path argument. Tests: `the_daemons_monitor_and_its_served_create_action_share_one_writer` (the monitor's handle holds the wallet's writer; 100 rounds of the monitor's pass beside the served path); `the_daemon_tenant_opens_its_file_only_through_open_shared` and `no_command_opens_the_wallet_file_beside_its_store` (`src/store.rs`, the sources read: the daemon tenant opens its file only through `store::open_shared`, `daemon`, `serve` and `serve-fleet` open nothing themselves, and no file in `src/commands/` opens the wallet file except `init`, `tick` and `ui`, which carry no Wallet, once each).
- `--json` refusals (F4 of #32): `send`, `drain`, `split` and `gift-send` print the store busy as `{code: STORE_BUSY, message}` and exit 1, as they print the gate's refusal. Tests: `a_busy_store_under_json_prints_store_busy_and_exits_1` (the binary, another connection holding the file's write lock), `the_gate_error_is_the_store_busy_by_its_type_only`.
- The witness: `create_action_beside_a_monitor_shaped_writer_answers_no_store_error` (`src/spend_gate/tests.rs`), e0ff758's reproduction un-ignored. A monitor-shaped writer (read, write, commit) on its own store's `write_tx` over the same file, as a second process on the file holds its own store, beside 300 rounds of the served handler's path (the spend guard, the gate's `createAction`, the abort): zero store errors, no writer transaction failed (four runs, 58,854 to 72,464 writer commits). On 0.10.0 every run failed within four rounds with 517 or 5. Beside it, `a_busy_before_the_door_answers_503_with_nothing_posted_and_the_coins_unchanged`.
- `served_beef_doors`' `inputBEEF` test gives its tracker the BEEF's root: the toolbox reads a caller's roots before any allocation now.
- Mixed versions on one wallet file are unsupported. Stop every 0.15 process on a file (`daemon`, `serve`, a CLI spend) before a 0.16 process opens it: the heal now runs mid-life too (a CLI spend's `heal_if_held`, a served request's owed retry), not only at start, so a 0.16 process can restore a running 0.15 process's in-flight hold in `spend_gate_holds` and offer that coin while the 0.15 action still spends it.
- The MCP crate (`mcp/`) is unchanged.

## [0.15.0] - 2026-10-20

The CLI on bsv-wallet-toolbox-rs 0.10.0, the reorg below the user in the toolbox (the stack's rulings 5 and 7 of 2026-10-20, bsv-stack-lean `docs/RULINGS-2026-10-12.md` 19:4xZ to 20:1xZ; `docs/DECISIONS-2026-10-19.md` section 8c). The `reproof` verb's output and the status word `actions` prints change, so a minor.

- `bsv-wallet-toolbox-rs` `=0.9.0` to `=0.10.0` (the lock moves that one entry). `bsv-rs` stays `=0.7.0` (0.10.0 asks `0.7.0` by caret) and `bsv-tracker` `=0.7.0`.
- `reproof` on the new API. A stored proof is stale when, and only when, its root is not the active header's root at its height (ruling 5); with `--execute` the header is read again for each row and decides, at most `REPROVES_PER_PASS` (4) a run, the rest postponed. A kept root moves nothing; a provider's path for the active block replaces the proof; otherwise the proof is demoted (rulings 7a, 7b: the action `unproven`, its inputs kept, nothing released or failed) and the courier is asked once (7d). The witnesses, `Deferred` and its count are gone. The table's action column reads `replaced by <height> <hash>`, `kept: the active header carries the stored root`, `confirming: demoted to unproven, nothing released; <what the courier said>`, `postponed to the next run` or `retained: <fault>`; the summary reads `executed: N replaced | N confirming (demoted, N announced once) | N kept | N postponed | N retained on error`. The JSON report loses `deferred` and gains `announced` and `postponed`. Tests: `execute_reads_the_header_again_demotes_a_moved_root_and_keeps_a_kept_one`, `execute_reproves_a_few_a_run_and_postpones_the_rest`.
- `actions` prints each action's status word; `unproven` (a proof not yet stored, and again after a reorg took it away) reads "confirming" (ruling 7b (b)). `--json` keeps the wire word. The daemon's served tier (`GET /tx/{txid}`) is 0.14.0's. Test: `an_unproven_action_reads_confirming`.
- One rule for the follow-up of a `stale` episode, not two. Toolbox 0.10.0's monitor re-asks the courier once after its demotion (`reask_lost_proof`), and 0.14.0's served pass did the same for the tracker's `stale` word, so a daemon could read the status twice and announce twice for one episode. Where the monitor runs its reorg or review task (`daemon`, a tenant of `serve-fleet --daemon`) the follow-up is the monitor's: the pass marks the row `monitor` (`tracker_states.follow_up`) and reads and announces nothing (`TickOptions::monitor_runs`; the summary counts them as "the monitor's"). Under `serve` and `tracker-tick`, which run no monitor, the pass's own follow-up of 0.14.0 runs unchanged. Test: `in_the_daemon_a_stale_episode_is_followed_up_once_by_the_monitor_not_twice` (one status read, no announce, across the monitor's path and two passes).

## [0.14.0] - 2026-10-20

The daemon on bsv-tracker 0.7.0 under the stack's rulings (5), (2) and 7 of 2026-10-20 (bsv-stack-lean `docs/RULINGS-2026-10-12.md`, 19:4xZ to 20:1xZ; `docs/DECISIONS-2026-10-19.md` section 8c; the draft `corpus/scenarios/2026-10-20-reorg-below-the-user.md`). The daemon's observable words change, so a minor. `bsv-wallet-toolbox-rs` stays `=0.9.0` (no newer release on crates.io at the start of the lane) and `bsv-rs` `=0.7.0`.

- `bsv-tracker` `=0.6.0` to `=0.7.0` (the lock moves that one entry). A reorg's `stale` is by the root: the crate rechecks each named word's stored root against the active header at its height. The daemon feeds the event with the new branch's headers: a height where the header service still answers a header the reorg deactivated has no answer, so the word stands suspect with `recheck` until the next tip, never `stale` by the height alone and never `mined` on the old header. A reorg whose new block carries the same root moves nothing and fetches nothing. A row keeps its reorg and replays its stored proof against the header the reorg took away, so a `stale` word stands with `reorg@<forkHeight>` across passes until a new proof; until 0.14.0 the next pass dropped the path and read `proofFailed`.
- With the feed held (`daemon`, a tenant of `serve-fleet --daemon`), the pass keeps no ring of tips and rereads no mined row (`TickOptions::feed_held`): the recheck is the crate's, once. `serve` keeps the ring. The words whose root moved are asked before any other re-ask, at most four a pass (`REORG_ASKS_PER_PASS`).
- The follow-up of a `stale` word (ruling 7d (c)): once per episode, the status read at the broadcasters (`get_broadcaster_statuses`); a broadcaster that knows it, no announce; every one that answered not knowing it, one announce of the stored BEEF, never a second; a refusal the node gave writes the node's verdict (`rejected`, or `conflicted` with the named competitors), stored on the row (`tracker_states.verdict`) and replayed. The episode's mark is `tracker_states.follow_up`. The pass's summary counts them.
- `GET /tx/{txid}` serves a `stale` word with `tier` (the word its hints give: `announced` or `seen`, read as "confirming"; ruling 7b (b)) and its re-ask. Every other answer keeps its shape.
- Ruling (2): a stored courier presence hint (`mempool_present`) on a `built` transaction now replays to `announced`.
- Tests: `step_3_a_reorg_before_the_service_moves_leaves_the_word_suspect_until_the_next_tip`, `a_reorg_whose_new_block_carries_the_root_moves_nothing_and_fetches_nothing`, `step_4_i_a_broadcaster_that_knows_the_stale_transaction_is_not_told_again`, `step_4_ii_a_stale_transaction_no_broadcaster_knows_is_announced_once` (with step 7), `a_refused_announce_of_a_stale_transaction_writes_the_nodes_verdict` (`src/chain_feed_tests.rs`), `a_stored_presence_hint_on_a_built_transaction_replays_to_announced`, `the_route_serves_a_stale_word_with_its_tier_and_its_reask`. No witness of `tracker_words_witnesses.rs` moved: none of its words is a reorg's or a presence hint's.
- The paved road's deny gate (bsv-stack-lean #105, wave lane W17): `deny.toml` is the stack's `template/deny.toml` byte for byte (sha256 `cdad82c09e2c88aa8315c7bf1a26172753187ada58936a2480670d3c9afc05ee`), `template/stack.toml` carries the line's `[deny]` table at bsv-stack-lean `c319cf6`, and CI gains the `deny` job (the hash check, then cargo-deny 0.20.2 from a temporary root, `--locked check`). The required contexts gain `deny` in the protection write, not in this commit.
- Four advisories cleared in the lock by each dependency's own release on crates.io, no manifest change: `anyhow` 1.0.102 to 1.0.104 (RUSTSEC-2026-0190, unsound), `h2` 0.4.13 to 0.4.20 (RUSTSEC-2026-0258), `event-listener` 5.4.1 to 5.4.2 (RUSTSEC-2026-0221, unsound; `concurrent-queue` leaves the lock), `rand` 0.8.5 to 0.8.8 (RUSTSEC-2026-0097, unsound). `cargo deny --locked check`: advisories ok, bans ok, licenses ok, sources ok.

## [0.13.0] - 2026-10-18

The line of the wave of 2026-10-18 (bsv-stack-lean #119): `bsv-rs` to `=0.7.0`, `bsv-wallet-toolbox-rs` to `=0.9.0` and `bsv-tracker` to `=0.6.0`, together, because the toolbox and the tracker put bsv-rs types in their public APIs. The committed lock carries one SDK version, bsv-rs 0.7.0. Supersedes the update loop's #35 (tracker 0.4.1).

- bsv-tracker 0.5.0 widens `HintStatus` by `MempoolPresent { source }`. The stored hint row (`tracker_states.hints`) writes it as `mempool_present` with its courier in a new optional `courier` field, and reads it back; a `mempool_present` row without a courier reads as `unknown`. The CLI produces no such hint yet. The tracker's compact State form (`State::to_compact`, `from_compact`, `Tracker::restore`) is not adopted: the CLI stores a row of inputs that every load replays (whether built, the hints in order, the raw merkle path, the chain events), never the tracker's state, so its stored path is checked again against the headers on each load; the compact form would store the word.
- bsv-rs 0.7.0's signing digest (the node's dispatch on FORKID and CHRONICLE, the SINGLE sentinel, an explicit scriptCode). The CLI signs on two paths of its own, both with FORKID and without CHRONICLE, where the digest is unchanged: the gift claim's `compute_sighash_for_signing` (scopes 0x41 and 0xc3) and the respend's `P2PKH::unlock` through `Transaction::sign`. Every other signature is the toolbox's `wallet::signer`, which computes its own digest. `tests/signing_digest_vector.rs` pins both paths to bsv-script-lean v1.1.1 `corpus/signer/p2pkh.jsonl` row `signer-p2pkh-00001`, the row the toolbox 0.9.0 test `the_digest_the_toolbox_signs_with_is_the_nodes_for_a_p2pkh_all_forkid_spend` pins: the digest `7370…06b3` and the signed spend's txid `e418…7a96`, the control column of the row's record, judged valid by bitcoin-sv v1.2.3 `verifyscript` (cited, not re-run).
- `bsv-wallet-mcp` is unchanged (0.3.0, 34 tools); it depends on none of the three crates.

## [0.12.0] - 2026-10-18

From the stack's coin read (bsv-stack-lean #164, findings F1 to F4 and F6; Calhooon/bsv-wallet-cli #32).

- `daemon` runs the served pass `serve` runs (`run_served_pass`: the tracker's pass, the sweep, the spend guard), every `BROADCAST_RECONCILE_INTERVAL_SECS` (default 60), with no SSE drain (the monitor holds Arcade's stream). In 0.11.0 the daemon ran the feed but no tracker pass, so a daemon host's creators had no rows (F1).
- A proven creator adopted: a `completed` transaction with a stored proof (`proven_tx_id` set) that creates a coin we hold and has no tracker row is adopted by replaying its stored path against the headers of the moment: `mined(h)` with that path as evidence when the root checks, else no word and the re-ask `proofFailed` (never `rejected`, never a write to the toolbox's rows); at most 100 per pass. A creator the toolbox later completes keeps its row. `GET /tx/{txid}` answers for every coin the wallet holds. The pass's summary counts them.
- An open split holds its coins: the feed records each `split` (key, `forkHeight`) in `tracker_meta` `open_splits` in the delivery's transaction and drops it at its `splitResolved`; the guard holds every coin whose creator is `mined` at or above an open split's `forkHeight` (the reason `suspect: a fork or an unresolved split at or below its height`). The guard's recheck alone cleared the tracker's suspect mark while the header service still carried the root, so a split held nothing (F2; the scenario `tracker-split-observation-writes-no-word.md` column 4).
- The proof backstop's provider at info: each proof the toolbox monitor's `check_for_proofs` stores is logged with the provider that served its merkle path (marker `proof_backstop_provider`, `provider`, `txid`, `height`; `status-answer` when the triage carried it), read from the toolbox's own debug and info lines by a tracing layer; the README lists the site with the rule-28 rows (F3). No setting turns the backstop's explorer providers off.
- `/chain-feed`'s `status` is rewritten after every delivery (N = 1): its `cursor`, `delivered` and `updatedAt` follow the stored cursor while following (F6).
- `send`, `drain`, `split` and `gift-send` under `--json` print a refusal as the route's body, `{code, message, offeredSatoshis, held}`, on stdout, exit 1 (F4).

## [0.11.0] - 2026-10-18

- R1 (d) at the door (Calhooon/bsv-wallet-cli #29; bsv-stack-lean #168; the owner's ruling of 2026-10-18 00:1xZ): the served `/createAction` reads `allowPending` beside `options` (the one value `"received"`; absent or anything else is off; the toolbox's `CreateActionOptions` is unchanged), and `send`, `drain`, `split` and `gift-send` take `--allow-pending received`. Under it, and only for that action, the spend guard offers a received coin whose creator's word claims nothing about the chain (`unknown`, `built`, `announced`, `seen`); its body was checked at internalize (toolbox 0.8.0 `internalize_action.rs` 96-119). No environment variable. The action records the word in `spend_gate_actions` (txid, `received-pending`, when); `actions` and `/listActions` show it as `underRule`. A deferred-signing template (`signAndProcess: false`) has no txid yet: its row is recorded by the action's `reference` (a nullable `reference` column, txid nullable, one of the two set), the served `signAction` fills the txid it was signed as and keeps the reference, the listing shows `underRule` on the template by its reference (where no unsigned template of the same status, description and satoshis carries another rule or none) and on the signed action by its txid, and `abortAction` leaves the row. A held received pending coin's reason ends `offered under allowPending: received`. The cost: a double-spending sender makes the action `conflicted`.
- `GET /tx/{txid}`: the tracker's stored word for one transaction, `{txid, word, height, reask, evidence}` (`evidence` the stored proof's root and height for `mined`, else `null`), behind the wallet's auth; 404 for an unknown txid. `bsv-wallet-mcp` 0.3.0 restores the `tx_word` tool over it (34 tools) and gives `create_action` the `allowPending` parameter.
- Explicit inputs under the guard: coins named in `createAction`'s `inputs` and `respend`'s exact inputs pass the same per-coin judgment as the toolbox's allocation; a named coin the guard holds refuses the action (`403 INPUT_NOT_OFFERED`, the same `held` list). A respend inherits the word its phantom was created under.
- The frozen record: the feed's `frozen` event writes its outpoint to `frozen_outpoints` (txid, vout, cursor, when) in the delivery's transaction; the guard never offers such a coin (`frozen: never offered`; never cleared here, R6); `GET /chain-feed` gains `frozenCount`.

## [0.10.0] - 2026-10-18

- The spend guard on the binary's real coin selection (Calhooon/bsv-wallet-cli #28; the stack's wallet charter R1 (b) and R3 (b), rulings of 2026-10-12): every coin the served `/createAction`, `send`, `drain`, `split` and `gift-send` offer to the toolbox's `createAction` is either mined at `SPEND_DEPTH` (floor 1) under the headers of the moment, or our own `made` change under `SPEND_OWN_CHANGE` (new, default `1`). A received coin with no proof is never offered. The toolbox allocates inside its own query, so the guard holds the coins it does not offer (`outputs.change = 0`, recorded in `spend_gate_holds`) for the call and restores exactly those after; a hold left by a call that did not finish is restored by the next one after ten minutes. Departs from the reference's allocation of unproven change (ts-stack `edf6e03` `StorageKnex.ts:2222-2231`).
- The refusal lists every coin held back, with its word, origin, age in minutes and the rule that holds it; `/createAction` answers `402 INSUFFICIENT_FUNDS` with the list under `held`.
- `bsv-wallet-mcp` 0.2.0: rmcp 3.5; four tools over the host's existing routes, `chain_feed_status`, `health`, `pull`, `register` (the relay's at `RELAY_URL`, `RELAY_ADMIN_TOKEN` for `register`). The README says how the binary is built (`cargo build --release -p bsv-wallet-mcp`, in the workspace's `target/release`) and gives a client config. Not in this release: a route serving the tracker's word for one transaction with its evidence, and its tool (#29).

## [0.9.0] - 2026-10-17

- `daemon` holds the header service's chain-event feed (`CHAINTRACKS_URL`; Calhooon/bsv-wallet-cli #26, bsv-stack-lean #167): a replay through `GET /events?since=` from the stored cursor until `hasMore` is false, then the SSE stream; every envelope through `bsv_tracker::decode_envelope` and `Tracker::on_chain` with the daemon's headers; the cursor stored after each delivery in the same transaction as the tracker's rows (`tracker_meta`, `feed_cursor`). A decode fault, `cursorAhead` (409), an `event: error` or a refused request stops the loop at its cursor and is reported at `GET /chain-feed`; a dropped stream reconnects (the page from the cursor, then the stream with `Last-Event-ID`). Load: one stream per daemon plus one page request per reconnect.
- A chain event that leaves a tracked transaction other than a plain checked `mined` (a reorg's `stale`, a fork's or a split's suspect mark) is kept on its row (`tracker_states.chain`, added to an existing table empty) and replayed on each load; a new proof, or a stored path dropped as no longer checking, clears it.
- `SPEND_DEPTH`: the spend depth the tracker host's spend guard reads, default 1 and the floor (the stack's wallet charter R3, `lean/Wallet.lean` `guardDepth`).

## [0.8.0] - 2026-10-17

- Pin `bsv-wallet-toolbox-rs` to `=0.8.0`, `bsv-rs` to `=0.6.1` and `bsv-tracker` to `=0.4.0`. They move together because toolbox 0.8.0 and tracker 0.3.0 put bsv-rs 0.6 types in their public APIs. The committed lock carries one SDK version, bsv-rs 0.6.1, on the line the toolbox's caret `0.6.0` admits.
- bsv-tracker 0.4.0 adds two `ChainEvent` variants, `Split` and `SplitResolved`. The CLI has no exhaustive `match` on `ChainEvent` because it does not decode the header feed, so neither variant needs an arm and no split moves a coin.

## [0.7.5] - 2026-10-12

- Pin `bsv-wallet-toolbox-rs` to `=0.7.6`, `bsv-tracker` to `=0.2.2`, and `bsv-rs` to `=0.5.0`. The committed lock carries one SDK version.
- Report a refused courier BEEF with its transaction id, courier and validation reason in `receive` and `sync`. `sync` processes the remaining outpoints and exits nonzero if any were refused. Incomplete unmined stubs are refused before storage by toolbox 0.7.6 ([toolbox #23](https://github.com/Calhooon/bsv-wallet-toolbox-rs/issues/23)).
- Add mock courier regressions for refusal without storage and continued sync processing. Completing missing ancestry through a courier remains separate work.

## [0.7.4] - 2026-10-12

`respend <txid>`: the host's deliberate competitor for a transaction a broadcaster accepted that never mines (bsv-stack-lean #67; the tracker rule: no hand writes a status, the chain decides). The L2 re-pin: bsv-wallet-toolbox-rs 0.7.5, bsv-tracker 0.2.1, bsv-rs 0.4.5 (the lone leaf at offset 0 only; the LOCAL work bound inside one input's script); no behavior change under the defaults.

## Unreleased

- Pin `bsv-wallet-toolbox-rs` to `=0.7.5` and `bsv-rs` to `=0.4.5`.
  bsv-rs 0.4.5: the lone leaf at offset 0 only; the LOCAL work bound; no behavior change under the defaults.

- Add `respend <txid>`: deliberately spend an accepted transaction's exact
  wallet-owned inputs back to wallet change, using the wallet fee policy.
  Refuse while any answering status source holds the original, when status
  lookups fail, after proof, or for an input the wallet cannot sign.
  The replacement follows the toolbox process and broadcast path and is
  retried on its cadence. Construction and broadcaster hints leave the
  original's word and input reservations intact; only a checked competitor
  proof writes `doubleSpend`. A served pass assigns locked inputs to the
  proven replacement; the spend guard keeps those inputs reserved even
  when an unspent hint reaches the rollback path. A recorded build interrupted before processing is
  completed by that pass; repeated construction is refused.
  No change to the age policy for accepted unproven change, dependencies,
  or crate version (bsv-stack-lean #67).

## 0.7.3

The toolbox 0.7.4 (bsv-stack-lean #66: every broadcaster word is a hint; a
refusal on the immediate post is not an error), and the host's side of it:
no path of the CLI retires a transaction a broadcaster took, the served
doors answer a refused post with the tracker's word, and a coin whose
transaction drew no accepting word is not spent.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.4 (was 0.7.3). `fastrand`
  2.5.0 and `spin` 0.9.9 (were 2.4.0 and 0.9.8, both yanked, in the lock
  since 0.7.1): a lock-only move.
- **The vector.** `tests/vectors/arcade_status_verdicts.json` is the toolbox
  0.7.4's file, byte-identical (`cmp` against the crate's file); four
  `toolbox_push` values move to `none` (`push_rejected_467`,
  `push_rejected_466_conflict`, `push_double_spend_attempted`,
  `push_rejected_lower_case`): a pushed REJECTED or DOUBLE_SPEND_ATTEMPTED
  writes no word.
- **No retire of a transaction a broadcaster took** (the tracker's
  `abandoned` is the host's act on a template never announced; #24). An
  accepting word in the wallet's records (the proof request past the post,
  an `accepted`/`seen`/`mined` memory row, the tracker's word past `built`)
  keeps a transaction from every retire path: the served sweep (which
  retired any unproven transaction with a `rejected` memory row, now a
  hint), `reconcile-broadcasts`, `cleanup-abandoned` and the daemon's
  ticker (which abandoned one the broadcaster held past
  `BROADCAST_ABSENCE_MINUTES`, or found nowhere), the after-send follow-up,
  and `/abortAction` (409). It stays, re-asked, until a proof, a node
  verdict or a competitor's checked proof. A transaction no broadcaster took
  is retired as before; a confirmed competitor's spend still abandons.
- **The doors' word for a refused post.** "Transaction broadcast failed" no
  longer reaches the CLI, and the `BROADCAST_REJECTED` mapping that promised
  "the tx is already failed and its inputs released" is gone.
  `/createAction` and `/signAction` answering `sending`, and
  `/internalizeAction`, carry `broadcast: {word: "built", request, hint,
  inputs: "locked"}` (the request's status, the toolbox's
  `immediateBroadcastHint`); the MCP tools say the same. The inline
  follow-up's 502 promises a release only where the chain vouches the coin.
- **The spend guard.** A coin of a transaction whose immediate post drew no
  accepting word, with no status source holding it, is held (not
  spendable, recorded in `spend_guard_holds`) before every spend and on each
  served pass, and released once the toolbox promotes the request
  (`unmined`) or a proof completes it; a retire drops the hold.

### Upgrade

The wallet database gains `spend_guard_holds`; no existing table changes. A
held coin reads `spendable = 0` until released; the toolbox's balance does
not count it meanwhile. A client of the served doors that read 502
`BROADCAST_REJECTED` on a refused post now receives 200 with `sending` and
`broadcast.word = "built"`, and reads the refusal there or from the
monitor. A daemon that relied on the absence threshold, or on
`/abortAction`, to free the inputs of a transaction a broadcaster accepted
keeps them locked. The library's report structs (`ReconcileBroadcastsReport`,
`SweepReport`, `ServedPassReport`) and the doors' response types gain a
field each. Rollback: pin 0.7.2 (with the toolbox 0.7.3); `spend_guard_holds`
is left unread, and a coin it held stays `spendable = 0` until set back by
hand (`UPDATE outputs SET spendable = 1 WHERE output_id IN (SELECT
output_id FROM spend_guard_holds)`).

## 0.7.2

The toolbox 0.7.3 and bsv-rs 0.4.3, moved together; no line of the CLI's
code changed, one test added.

- **Dependencies.** `bsv-wallet-toolbox-rs` 0.7.3 (was 0.7.0) and the CLI's
  own `bsv-rs` (`bsv-sdk`) 0.4.3 (was 0.4.1), in one commit, so the graph
  holds one bsv-rs; `bsv-tracker` 0.2.0 resolves the same 0.4.3.
- **A transaction with no output is invalid bytes** (bsv-rs 0.4.3,
  bsv-stack-lean #59; the node's rule, bsv-script-lean@87f0461
  `lean/BsvScript/TxRules.lean:85-86`), as one with no input has been since
  0.7.0. The CLI's reader of a stranger's BEEF (`atomic_beef`, the
  toolbox's `refuse_invalid_beef_bytes`, which the served doors also call)
  refuses one at the transaction's offset, kind `NoOutputs`, under a BUMP
  too; the witness `a_transaction_with_no_output_is_refused_at_its_offset`
  fails on 0.7.1's dependencies (the BEEF taken) and passes here. No fixture
  of the CLI carries a transaction with no output, and no code of the CLI
  matches on bsv-rs's `Kind` or `Reason`, so the new variant breaks no match
  here.
- **What the toolbox 0.7.1 to 0.7.3 brings to the daemon.** A txid-only
  entry is valid only when the BEEF proves it, except that `createAction`
  with `trustSelf: 'known'` (the wallet's default) resolves an entry the
  wallet's own storage holds before it verifies. ARC is posted the plain BEEF
  as written, and ARC's 400 is a request fault that schedules a re-ask, not
  the transaction's rejection. The monitor retires no announced transaction
  on transient words or by age: a `sending` transaction stays `sending`, its
  inputs locked, re-asked on a 1, 2, 4 ... 64 minute cadence, until a
  definitive word, a proof, or the host's explicit retire.

### Upgrade

Nothing stored is migrated. A daemon that relied on the toolbox failing a
transaction stuck in `sending` (after seven attempts, or five minutes) now
keeps it announced and its inputs locked; the CLI's own retire paths
(`broadcast_reconcile`'s absence threshold, `/abortAction`, the release rule
on a definitive rejection) are unchanged. Rollback: pin 0.7.1.

## 0.7.1

- **No open bind without a token.** `serve`, `daemon` and `serve-fleet`
  refuse at startup, before the wallet is opened or a socket bound, to bind
  an address beyond loopback (`BIND_ADDR`, IPv4 or IPv6) when no bearer token
  (`AUTH_TOKEN`, an empty value counting as none) is set. The error names the
  one flag, `--allow-no-token`, that permits the open bind deliberately. A
  loopback bind without a token is unchanged.
- **The risk it closes.** Since 0.7.0 the served `/internalizeAction` and
  `/createAction` doors take a body of any size, auth before the body; a
  daemon bound to `0.0.0.0` with no token was a wallet any host on the
  network could drive, and made to hold any body it sent.

### Upgrade

A daemon started with `BIND_ADDR` beyond loopback and no `AUTH_TOKEN` now
exits non-zero at startup: set `AUTH_TOKEN` (and send it as
`Authorization: Bearer`), or add `--allow-no-token` to keep the open bind.
The library gains `server::refuse_open_bind`,
`server::refuse_open_bind_from_env` and `server::bind_addr_from_env`;
`server::run` itself does not check, so a host calling it directly checks
first.

## 0.7.0

The 0.4 line: bsv-rs 0.4.1 under the CLI, the toolbox and the tracker, one
copy in the graph, and the posture that a valid BEEF is never refused for its
size or its counts; a BEEF is refused only for invalid bytes, at their offset.

- **Dependencies, moved together.** `bsv-wallet-toolbox-rs` 0.7.0,
  `bsv-tracker` 0.2.0 and the CLI's own `bsv-rs` (`bsv-sdk`) 0.4.1. The
  tracker 0.2.0 takes bsv-rs 0.4's `MerklePath`, so the three move in one
  commit; with any one left behind the graph holds two bsv-rs copies and the
  CLI does not compile.
- **A transaction with no input is invalid bytes** (bsv-rs 0.4.1). A BEEF
  carrying one is refused at the transaction's leading byte (`NoInputs`),
  where 0.6.0 internalized it when no chain tracker was set.
- **The commands' BEEF funnel.** `fund`, `receive` and `gift-claim` hand a
  stranger's BEEF to the wallet through `atomic_beef::ensure_atomic`, which
  now reads it with the streaming reader first: a refusal names the byte in
  the caller's own bytes and the reader's kind (`Invalid BEEF at byte N:
  Kind`). `receive` reads the courier's bytes the same way before it matches
  an output.
- **The served wallet's BEEF doors.** `/internalizeAction` (`tx`) and
  `/createAction` (`inputBEEF`) no longer refuse a body over 50 MiB (0.6.0:
  413 `TOO_LARGE`, a valid BEEF of about 13 MB and more refused for its
  size); every other route keeps the 50 MiB cap. An invalid BEEF is 400
  `INVALID_BEEF`, the message naming the offset and the kind.
- **Auth before the body.** The bearer token is checked before any request
  body is read, so the uncapped doors are open only to the wallet's caller.

### The bounds that stay, named

- The served doors hold the JSON body whole (the BRC-100 JSON wire carries
  the BEEF as one array, about four bytes of body per byte of BEEF): memory
  linear in the BEEF, never a refusal. With no bearer token set (the default,
  bound to 127.0.0.1), any local caller can send a body of any size.
- The commands hold a BEEF whole: a hex argument (`fund`), a courier's
  response body (`receive`), the toolbox's argument (`internalizeAction`'s
  `tx`, one `Vec<u8>`).
- `/arc-callback` and the relay keep a 1,000,000-byte cap; they carry a
  broadcaster's status and a merkle path, never a BEEF.

### Upgrade

The library's public API names the toolbox's and bsv-rs's types
(`server::make_wallet_state` takes the toolbox's `Wallet`; `tracker_host`
binds the tracker's and bsv-rs's), so a host linking `bsv_wallet_cli` moves
to the toolbox 0.7.0 and bsv-rs 0.4.1 with it, and one matching on the
toolbox's `Error` meets `Error::InvalidBeef { offset, kind }`. A payer whose wallet emits a BEEF with an input-less transaction is
refused where 0.6.0 credited it; the node would not mine such a transaction.
