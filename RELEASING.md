# Releasing bsv-wallet-cli

1. Every change reaches `main` through a pull request; `main` is protected, admins included, linear history.
2. The PR merges (rebase-merge) once its required checks are green: `Test`, `Rustfmt`, `Clippy`, `vectors`, and the two `Build` checks.
3. A release is a PR like any other: bump `version` in `Cargo.toml` (and `Cargo.lock` with it), merge it green.
4. Tag the merged commit on `main`: `git tag vX.Y.Z <sha> && git push origin vX.Y.Z` (the tag must equal `Cargo.toml`'s version).
5. The tag runs `.github/workflows/release.yml`: it checks the tag against the manifest and that the commit is on `main`, re-runs the CI jobs, then `cargo publish --dry-run` and `cargo publish`, and beside them builds the binaries and creates the GitHub release.
6. Nothing publishes from a laptop; no crates.io token is stored anywhere, in this repository or its secrets.
7. Publishing uses crates.io trusted publishing: the crate's Trusted Publishing setting names owner `Calhooon`, repository `bsv-wallet-cli`, workflow `release.yml`, no environment.
8. A failed release job publishes nothing; fix through a PR, then the owner moves the tag to the fixed commit, or tags the next patch.
9. A published version is never re-published; a bad release is yanked on crates.io and superseded by the next patch.
10. Without the trusted-publishing setting on crates.io the publish step fails closed; the binaries still build.
