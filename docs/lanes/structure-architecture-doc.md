# Lane structure-architecture-doc: docs/ARCHITECTURE.md, the dataflow and the invariants (#83)

**Repository.** `bsv-wallet-cli`, your worktree on a branch from `origin/beta`. `docs/lanes/README.md` binds you. Three hours.

**Read in this order.** #83 and the epic #106. `CLAUDE.md`, `README.md` (the sections "The spend guard on coin selection", "The served answers", "Explorer calls (rule 28)", "The tracker pass", "The chain-event feed"), `src/main.rs` (the module list and the command routing), `src/server/mod.rs` and `handlers.rs` (the door), `src/spend_gate.rs` (its module doc is the gate's contract), `src/tracker_host.rs` (its module doc and the header snapshot), `src/broadcast_verify.rs`, `src/broadcast_reconcile.rs`, `src/explorers.rs`, `src/commands/daemon_tenant.rs` (the task table), `src/chain_feed.rs`. The stack's `~/bsv/bsv-stack-lean/docs/charters/wallet.md` (sections 1, 2, 5, 6), `tracker.md`, `broadcaster.md`, read with `git -C ~/bsv/bsv-stack-lean show origin/main:<path>`; cite them by section, never restate a rule.

**The work.** `docs/ARCHITECTURE.md`, for an engineer who has not read the rulings corpus:
1. The dataflow of one spend, from the served door (or a command) through the guard, the gate's offer, the toolbox's allocation and signing, the broadcast, the verifier, the tracker's word, the feed, in one diagram (Mermaid in the file, rendered to `docs/img/architecture.svg` if a renderer is in the tree; else the Mermaid alone) and one paragraph per hop naming the file and the function.
2. The dataflow of one received payment (`internalizeAction`, `receive`, `fund`), and of one repair pass, the same way.
3. The invariants, each as one sentence, with the charter section it refines and the test that holds it today: the gate writes nothing around the door; an external fault is never a verdict; a negative needs two clean answers; the wallet reads its own records at the moment of decision; a coin a live action consumed is spent, never a hold; the word is per action and never a setting; the served shape is MetaNet's. Where no test holds an invariant, say so (it becomes an issue, named in the report).
4. The module map: every file under `src/` in one table with its one-line purpose and the machine of the stack it serves (wallet host, tracker host, broadcaster client, explorers, repair).
5. Where the next phases cut: a short section pointing at #78, #79, #80, #81, #86, #87, #88, #89 by name, so the reader knows what is about to move.

**Bounds.** Docs only; no edit under `src/`. No em dash. Every claim about behaviour cites a file and line or a test name; a claim you could not verify is marked `[D]`.

**Report.** `REPORT: STATUS done|blocked`, the base and tip commits, the invariants without a test, the commands and their tails (the Mermaid renders, if any), the six tests each choice passes.
