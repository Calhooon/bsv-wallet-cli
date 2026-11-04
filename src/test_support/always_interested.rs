//! The test binary's always-interested global default (#568). The respend
//! witness binary mounts this file too (`#[path]` in
//! `tests/respend_witness/main.rs`), since its capture lives outside
//! `test_support`: one copy for both crates.

/// Install [`AlwaysInterested`] as this test binary's global default, once
/// (#568). Every capture calls it before its own subscriber registers, and
/// a test that logs before any capture may call it first.
///
/// While at most one dispatcher is registered, tracing-core 0.1.36 asks only
/// the registering thread's default for a callsite's interest
/// (`callsite.rs:545-546`, `has_just_one` gives `Rebuilder::JustOne`, whose
/// `for_each` calls `dispatcher::get_default` at `:564-565`). A capture-less
/// thread that reaches a callsite first, while another thread's capture is
/// the only dispatcher, caches it as never, and the capture's own line at
/// that callsite is then skipped (#553). This one never dies (the global is
/// leaked, `dispatcher.rs:314-318`), so a capture registered after it makes
/// two live dispatchers (`callsite.rs:553-556`) and every rebuild after
/// that asks them all: a callsite's interest is every live dispatcher's,
/// combined (`:497-503`, `Interest::and`: equal stays, unequal is
/// sometimes), and a capture-less thread's default is this one, never none.
/// No callsite is cached as never; the thread-local captures only filter.
///
/// A second install (another binary path that set a global first) is
/// tolerated: the `Err` is the one already in place.
pub fn init() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let _ = tracing::subscriber::set_global_default(AlwaysInterested);
    });
}

/// A subscriber interested in every callsite at every level that writes
/// nothing: [`init`]'s global default. Not a `tracing_subscriber::fmt`
/// subscriber, whose writer would print every capture-less line of the
/// binary (and whose filter is what may say never); this one records
/// nothing at all.
pub struct AlwaysInterested;

impl tracing::Subscriber for AlwaysInterested {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }

    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        None
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}
