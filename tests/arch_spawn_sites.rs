//! Rule A6 (#73): a task is started from the daemon's task table, where its
//! lifetime and its shutdown are seen. `tokio::spawn` (and the other spawn
//! doors) in the shipped code lives in the task table files; every other
//! file that spawns today is listed with its count and why.

mod arch_support;

use arch_support::{count, hits, judge};

/// The task table: the daemon's tenant loop, the broadcast reconcile loops
/// it and `serve` start, and the server's router.
const TABLE: &[&str] = &[
    "src/commands/daemon_tenant.rs",
    "src/broadcast_reconcile.rs",
    "src/server/mod.rs",
];

/// The doors that start a task or a thread.
const DOORS: &[&str] = &[
    "tokio::spawn(",
    "task::spawn(",
    "spawn_blocking(",
    "spawn_local(",
    "thread::spawn(",
];

/// `(file, today's count, why)`. Only shrinks.
const ALLOWED: &[(&str, usize, &str)] = &[
    ("src/backstop_log.rs", 1, "the backstop log's writer task"),
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
        if TABLE.contains(&l.source.file.as_str()) {
            return 0;
        }
        DOORS.iter().map(|d| count(l.bare, d)).sum()
    });
    judge("A6: a task spawned outside the task table", &found, ALLOWED);
}
