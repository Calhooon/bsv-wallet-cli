//! A store whose busy answers at once, for the tests that hold its lock
//! to the end. The wire harness mounts this file too (`#[path]` in
//! `tests/wire_harness/worlds.rs`): one copy for both crates.

/// No busy wait on any connection of `pool`: its later connections open
/// with none, and every idle one is told so. One `PRAGMA` through the pool
/// reached one connection, and a read on another waited SQLite's 5,000 ms
/// out before its busy (#506; the wire harness's `answer_busy_at_once`,
/// #477).
pub async fn no_busy_wait(pool: &sqlx::SqlitePool) {
    let options = pool.connect_options().as_ref().clone();
    pool.set_connect_options(options.busy_timeout(std::time::Duration::ZERO));
    let mut idle = Vec::new();
    while let Some(mut connection) = pool.try_acquire() {
        sqlx::query("PRAGMA busy_timeout = 0")
            .execute(&mut *connection)
            .await
            .unwrap();
        idle.push(connection);
    }
}
