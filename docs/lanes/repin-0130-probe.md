# Lane repin-0130-probe: the CLI's 0.18.1 stable patch, on `main`

**The brief file.** The lane was told to read its brief at
`docs/lanes/repin-0130-probe.md` in the main checkout and copy it here as the
record. On 2026-10-23 that file did not exist: not in the main checkout
(`docs/` held `reading-a-balance.md` and `automerge-proof-2026-10-20.md`;
`docs/lanes/` existed on `origin/beta` only, with the oracle and structure
briefs and the lanes README), not on any local or remote ref of this
repository, not under `/Volumes/Lanes`. This record is the brief as the lane
read it from the lane's name and the toolbox's own release record, written
down so the captain can judge the reading before landing.

**The reading.** `repin-0130-probe`: pin `bsv-wallet-toolbox-rs` `=0.13.0`
(published 2026-10-23, tag v0.13.0 at e9857b8), whose one change is
`WalletServices::is_utxo_with_spender` (bsv-wallet-toolbox-rs #44, filed from
the review of bsv-wallet-cli PR 51, 0.17.0, finding L1). Its CHANGELOG names
the consumer's side of the work: "A caller that wanted the spender's name
(bsv-wallet-cli's `probe_input_spend`) asked WhatsOnChain's
`/tx/{txid}/{vout}/spent` and BananaBlocks' `/txo/{txid}/{vout}`, then read
the named spender's bytes, before `is_utxo` asked the same routes again: four
to six explorer requests an outpoint. On this call it drops its own step and
the probe costs one ladder pass." So: the pin moves, the spend probe asks the
one call and drops its own spender rungs, and the CLI's patch is 0.18.1 on
`main` (RELEASING.md 1: patches, fixes, pin moves and docs land on `main`; the
north-star programme lands on `beta`).

**Bounds.** The pin (`Cargo.toml`, `Cargo.lock`: the toolbox entry and this
crate's version alone), the probe and its callers, the explorers module's
spender rungs, the request-site test, the docs that name the probe's rungs
(README's rule 28 table and pin line, CLAUDE.md's ladder line, CHANGELOG).
No other behavior; no wire change; no new environment. No network but reads
(the crates.io index and the toolbox crate); no sats; no port of the
registry's. The rules of `docs/lanes/README.md` (origin/beta) bind, with the
base and PR target `main` by the captain's word.

**Left for the captain.** The PR into `main`, the tag `v0.18.1`, the publish,
the mainnet e2e gate, `main` into `beta`, HANDOFF.


**Landing note (the captain, 2026-10-23).** The cold review of PR 128 found two toolbox-side gaps the probe move would expose (an unreadable spender name falling through to the unspent hint; no spender named with the header service off), filed on the toolbox. The probe change was taken out of 0.18.1 at landing; 0.18.1 is the two pins (toolbox =0.13.0 and bsv-rs =0.8.2, the second added by the captain beyond this lane's bounds, recorded here). The probe moves in the patch that pins the toolbox fix.
