//! Rule A6 (#73): a task is started from the daemon's task table, where its
//! lifetime and its shutdown are seen. The spawn doors of the shipped code
//! (`spawn(`, `spawn_blocking(`, `spawn_local(`, by any path, as a method,
//! or bare after a `use`, and the `use` that brings the name in, one site
//! each) are counted file by file: the task table's own
//! sites (`src/commands/daemon_tenant.rs`, `src/broadcast_reconcile.rs`) by
//! their exact number, so a new task is a deliberate entry, and every other
//! file that spawns today with its count and why. A spawn through a helper
//! (`spawn_served_loop`) is the helper's one site, not its callers'.

mod arch_support;

use arch_support::{calls, hits, judge, words};

/// The doors that start a task or a thread, by name.
const DOORS: &[&str] = &["spawn", "spawn_blocking", "spawn_local"];

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/backstop_log.rs", 1, "the backstop log's writer task"),
    (
        "src/broadcast_reconcile.rs",
        2,
        "the task table: the reconcile loop daemon and serve start, and its SSE drain",
    ),
    (
        "src/commands/daemon_tenant.rs",
        4,
        "the task table: the chain feed, the abandoned reconcile, the relay poll, the server",
    ),
    (
        "src/commands/serve_fleet.rs",
        2,
        "each tenant's server and its reconcile loop",
    ),
    (
        "src/server/broadcast_follow_up.rs",
        1,
        "the served createAction's follow-up",
    ),
];

#[test]
fn tasks_start_only_from_the_task_table() {
    let found = hits(|l| {
        let doors: usize = DOORS.iter().map(|d| calls(l.bare, &format!("{d}("))).sum();
        let imports: usize = if l.is_use() {
            DOORS.iter().map(|d| words(l.bare, d)).sum()
        } else {
            0
        };
        doors + imports
    });
    judge("A6: a task spawned outside the task table", &found, ALLOWED);
}
