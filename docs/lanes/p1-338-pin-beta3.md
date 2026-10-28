# Beta.3 pin: dependency resolution record (#338)

The published package set cannot resolve. CLI [#341](https://github.com/Calhooon/bsv-wallet-cli/issues/341)
records the missing compatible tracker prerelease. The pin must resume at checklist
step 1 after that package reaches the registry. No dependency, lock, implementation
or fixture is changed by this record.

Base: `origin/beta` at `1d8f4dad32e946215b62606273e908ae06ad363f`.
Branch: `lane/p1-338-pin-beta3`. Model: `gpt-6.1-sol`.
The assigned worktree initially pointed at the main-line handoff `63cd391`;
it was clean and the lane had no remote branch or lane commits. Its starting
point was corrected to `origin/beta` before any project edit. The main checkout
was not used.

## Published package evidence

Read the downloaded packages' CHANGELOGs before any project edit, including
the entire bsv-rs beta.3 entry and toolbox beta.3 host note. Registry metadata
read at `2026-10-28T19:37:56.266699+00:00`:

| Package | Version | Package VCS commit | Dependency fact |
|---|---|---|---|
| bsv-rs | 0.9.0-beta.3 | `bf97cc74c2a778ded55fbbf243d27813115d4b83` | Requested SDK pin; published 15:30:17Z |
| bsv-wallet-toolbox-rs | 0.14.0-beta.3 | `aaaf2e2a88d4ab80daa148ffa643ca4cbeaffafa` | Package `Cargo.toml:220-224` requires bsv-rs `=0.9.0-beta.3`; published 19:34:31Z |
| bsv-tracker | 0.9.0-beta.2 | `109e9dd30c1f6186d4c91ff1a3ad1d78cdcd9fe8` | Package `Cargo.toml:72-75` requires bsv-rs `=0.9.0-beta.2`; newest published tracker |

The tracker index has no matching beta.3 release. Its SHA-256 at that read is
`cd6114c67d810be0dc955e39ea19c1cb2d4c3135c01e4bdb0a0a0c86e181ee9e`.
Leaving its existing pin, as the brief requires when no prerelease was cut,
leaves incompatible exact SDK requirements. Cargo cannot produce either lock.
No path patch, registry edit, relaxed constraint or substitute implementation
is used.

## Resolution proof

All probes ran in the foreground with a 3,600-second subprocess timeout and
`CARGO_TARGET_DIR=/Volumes/Lanes/targets/p1-338-pin-beta3`. Downloaded crates,
index snapshots, copied workspaces and logs are retained under
`/Volumes/Lanes/out/bsv-wallet-cli/p1-338-pin-beta3/`.

The minimal `pin-resolution/Cargo.toml` requests the two new exact pins and
the unchanged tracker pin with the CLI's dependency features. Command:

```text
cargo generate-lockfile --manifest-path /Volumes/Lanes/out/bsv-wallet-cli/p1-338-pin-beta3/pin-resolution/Cargo.toml
```

Tail, exit 101 (`pin-resolution.log`):

```text
versions that meet the requirements `=0.9.0-beta.2` are: 0.9.0-beta.2
all possible versions conflict with previously selected packages.
previously selected package `bsv-rs v0.9.0-beta.3`
failed to select a version for `bsv-rs` which could resolve this conflict
```

An archive of the actual base, in `workspace-resolution/`, reproduces it
with only the requested SDK and toolbox manifest pins changed. Both commands
exit 101, with the tracker named in the dependency chain:

```text
cargo update --manifest-path /Volumes/Lanes/out/bsv-wallet-cli/p1-338-pin-beta3/workspace-resolution/Cargo.toml -p bsv-rs --precise 0.9.0-beta.3
cargo update --manifest-path /Volumes/Lanes/out/bsv-wallet-cli/p1-338-pin-beta3/workspace-resolution/fuzz/Cargo.toml -p bsv-rs --precise 0.9.0-beta.3
```

Common tail (`root-workspace-resolution.log`, `fuzz-workspace-resolution.log`):

```text
error: failed to select a version for the requirement `bsv-rs = "=0.9.0-beta.2"`
candidate versions found which didn't match: 0.9.0-beta.3
location searched: crates.io index
required by package `bsv-tracker v0.9.0-beta.2`
```

## Intake reading, in comment order

The saved `issue-338.json` holds all 19 comments through the toolbox's ready
word. These checks record package readings, not implemented or replayed work.
Comment links are on [#338](https://github.com/Calhooon/bsv-wallet-cli/issues/338).

| Comment ID | SHA / surface | Package reading and remaining work |
|---|---|---|
| 6469912169 | `949f2782` through `3eb965cb`, SDK intake table | Beta.3 entry carries the listed interpreter, map, client, transport and validator changes; their CLI migration and replay await resolution |
| 6470094417 | `ff08faa3`, #417 signature words | Included; the entry separates default mode from explicit-word behavior |
| 6470655885 | `c02cb84`, toolbox #374 direct acquire | Host note lines 132-148 names the typed InvalidParameter refusal and bound-field text; fixtures 155-157 await replay |
| 6471243507 | `0ed2cd43`, SDK #427 linkage result | Included; host note line 61 also records the toolbox's corrected Anyone result |
| 6471644960 | `edfc442d`, SDK #416 transport bounds | Included; host note lines 41-47 names the six bounds and retained transport envelopes |
| 6471777280 | Owner's 19.7(b) release order | Both requested packages are published; the unannounced tracker incompatibility prevents the joint pin |
| 6471920031 | `421bec3`, toolbox #376 validator order | Included; host note lines 67-82 names the bad-label/bad-derivation text move |
| 6472128918 | `a53a9492`, SDK #429 default fork-ID behavior | Included; host note line 54 records the live internalize effect and separate toolbox #422 work |
| 6472508470 | `bf97cc74`, SDK tag | Matches the downloaded package's VCS metadata |
| 6472884128 | `08288cd`, toolbox #412 Dependents branch check | Included in host note lines 83-87; Dependents is not dispatched without a resolvable pin |
| 6472981493 | `1fb5a851`, SDK #408 after tag | Excluded, as host note line 65 states |
| 6473226561 | SDK registry hour | Registry confirms publication at 15:30:17Z; announcement observed it at 15:30:35Z |
| 6473255948 | SDK migration announcement | Client receiver and map changes are included; #379 and #408 are excluded; no BRC-56 HMAC fixture is moved |
| 6473298218 | Toolbox re-pin launch and landed items | Package source contains #353's fourth `must_spend: &[i64]` argument at `wallet.rs:1128-1135`, despite no dedicated #353 entry in the beta.3 host summary |
| 6473441573 | `1010d4de`, SDK #426 after tag | Excluded, as host note lines 47 and 65 state |
| 6474305964 | `dcbd639`, toolbox #383 description bounds | Included; host note lines 93-102 names both text moves |
| 6475204169 | Toolbox #421 review / #422 verifier mode | The final host note retains the default-mode effect and separate follow-up; no interpreter policy is changed here |
| 6477380058 | `28f565d`, toolbox re-pin landing | Included in the release ancestry; API adaptation still waits behind step 1 |
| 6478741077 | `aaaf2e2a`, toolbox ready word | Matches downloaded VCS metadata; sparse index publication is 19:34:31Z, announcement observation 19:35:30Z |

## Checklist boundary and resumption

- [x] Read the lane rules from origin/beta, the published package entries,
  all #338 comments, #330 and #313 with their comments, toolbox #353 and
  SDK #137 with their comments.
- [x] Step 1 attempted: minimal, actual root and actual fuzz dependency
  resolutions all exit 101. Neither project manifest nor lock is edited.
- [ ] Complete step 1 with a published compatible tracker and prove one of
  each stack crate in each lock.
- [ ] Steps 2-6: signatures, typed host map, --from, other intake items and
  the separate #331 boundary. These have no implementation in this record.
- [ ] Steps 7-8: fixture recording, replay, class table and release docs.
- [ ] Steps 9-10: the pinned build's full proof, single hermetic run,
  differential and same-build floor, and branch-tip Dependents dispatch.

No build, unit/integration test, wire recording, consumer or BRC replay,
gate-vector replay, scoreboard, ratchet, cargo-deny, hermetic suite or
differential is claimed. No server or watcher was started. No production
wallet, prohibited port or mainnet run was used. The implementation PR is
not opened before a resolvable pin exists; #341 carries the concrete next step.

Documentation-only checks for this record all exit 0:

```text
tools/check-links: check-links: 26 files, 0 broken links
tools/check-cites: check-cites: 1 files, 266 citations, 69 line-count rows checked, 0 findings
tools/changelog --check --base origin/beta: changelog: no watched paths changed
git diff --check origin/beta: no output
```

`Cargo.toml`, `Cargo.lock`, `fuzz/Cargo.toml` and `fuzz/Cargo.lock` were
compared to the base's git blobs and are byte-identical.
