//! Rule A5 (#73): an error is classified by its type, never by the prefix
//! of its message. `starts_with("..")` in the shipped code is refused
//! outside `AppError::classify` (`src/server/handlers.rs`), the prefix table
//! #81 replaces with typed errors and which is allowed whole until then.
//! Every other file that holds one today is listed with its count and why.

mod arch_support;

use arch_support::{count, hits, item_lines, judge};

/// The one function allowed whole, until #81.
const CLASSIFY: (&str, &str) = ("src/server/handlers.rs", "fn classify(");

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[(
    "src/commands/ui.rs",
    3,
    "the inspector's read-only statement check (SELECT, PRAGMA, EXPLAIN), not an error",
)];

#[test]
fn no_error_is_classified_by_its_prefix() {
    let found = hits(|l| {
        if l.source.file == CLASSIFY.0 {
            let (from, to) = item_lines(l.source, CLASSIFY.1).expect("AppError::classify");
            if (from..=to).contains(&l.number) {
                return 0;
            }
        }
        count(l.code, r#"starts_with(""#)
    });
    judge(
        "A5: a string prefix test outside AppError::classify",
        &found,
        ALLOWED,
    );
}
