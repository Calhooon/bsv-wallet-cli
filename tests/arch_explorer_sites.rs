//! Rule A3 (#73), rule 28 (`tests/rule_28_sites.rs`) extended to the
//! providers by name: the chain explorers (`WhatsOnChain`, Bitails,
//! `BananaBlocks`), their types and their hosts, are named in the shipped
//! code only in `src/explorers.rs` (the CLI's rungs, #50) and
//! `src/broadcast_verify.rs` (the chain indexes of C1). Every other file
//! that names one today is listed with its count and why. Comments are not
//! read; a name in a string literal is. A name built in pieces or read from
//! a setting is not seen (docs/ARCH-RULES.md, the known limits).

mod arch_support;

use arch_support::Rule;

/// The files that may name an explorer.
const HOMES: &[&str] = &["src/explorers.rs", "src/broadcast_verify.rs"];

/// The names, matched without case: the providers, the toolbox's provider
/// types (which carry them), and their hosts.
const NAMES: &[&str] = &["whatsonchain", "bitails", "bananablocks"];

/// `(file, today's names, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/commands/drain.rs",
        1,
        "a whatsonchain.com link printed for the user, no request",
    ),
    (
        "src/commands/gift_send.rs",
        1,
        "a whatsonchain.com link printed for the user, no request",
    ),
    (
        "src/commands/send.rs",
        1,
        "a whatsonchain.com link printed for the user, no request",
    ),
    (
        "src/commands/split.rs",
        1,
        "a whatsonchain.com link printed for the user, no request",
    ),
    (
        "src/commands/receive.rs",
        11,
        "the courier ladder's order and its provider names, its requests in src/explorers.rs",
    ),
    (
        "src/services_env.rs",
        46,
        "#80's ExplorerSettings beside the readers it replaces (13; the next commit removes the \
         readers and the count is 33 again); the explorers' env names (BITAILS_API_KEY, \
         BANANABLOCKS_BASE) and the break-glass warning (14); \
         the bases' configuration (#74, PR 131), 19: the toolbox's bitails and whatsonchain default-URL \
         imports (4), BITAILS_BASE_ENV and its \"BITAILS_BASE\" (2), ExplorerBases' bitails field, its \
         chain defaults and its overrides (13); no request is made here",
    ),
];

const RULE: Rule = Rule {
    name: "A3: an explorer named outside its rungs",
    homes: HOMES,
    allowed: ALLOWED,
    sites: |l| {
        let line = l.code.to_lowercase();
        NAMES.iter().map(|n| line.matches(n).count()).sum()
    },
};

#[test]
fn explorers_are_named_only_at_their_rungs() {
    RULE.check();
}
