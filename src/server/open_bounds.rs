//! The open doors' bound (#541, one of the three refinements of the
//! owner's W15 (i) ruling of 2026-11-03; `docs/design/origin-grants.md`
//! section "The open doors' bound"): a page the seed does not list, answered
//! at an open door or under a level-0 protocol with no row, is answered
//! within a bound per originator, so an admitted page cannot hammer them.
//!
//! The figures, the row's to change and never a page's:
//!
//! - **presence** (`isAuthenticated`, `waitForAuthentication`): one answer
//!   per door per originator a minute. The served auth layer keeps no
//!   session (the bearer is the owner's one token and never names a page),
//!   so the minute stands in for the page's session;
//! - **chain** (`getHeight`, `getHeaderForHeight`, `getNetwork`,
//!   `getVersion`): 60 a minute per originator, 10 at once, the four doors
//!   one bound;
//! - **level 0** (a protocol power at security level 0, at any door): the
//!   chain doors' figures, a bound of its own, charged once per call: each
//!   level-0 check reads the bound in the call's order, and the call moves
//!   it once when every check has admitted it (#556).
//!
//! Past a bound the call is answered at once, 429 `ERR_RATE_LIMITED`
//! naming the door and the window, with `Retry-After` in whole seconds
//! (the wait rounded up); nothing is written to the store. The counters
//! live in this process's memory (a restart resets them), keyed by the
//! originator's hostname (the grants' key: every port of a host one
//! originator). Each bound is the generic cell rate algorithm: a call is
//! admitted while its theoretical arrival time is at most the burst's
//! tolerance ahead of now, and each admitted call moves it on by one
//! emission interval.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::http::Method;
use tokio::time::Instant;

use super::error::{AppError, Kind};

/// The open doors (W15, ruled (i) by the owner on 2026-11-03): answered to
/// a page with a parsed `Origin` and an agreeing originator, with no row,
/// as the reference's manager gates none of them (`WPM:5492-5568`). Each
/// with the methods its route answers (the router's map in `make_router`;
/// `HEAD` rides `GET`): a call by another method is the router's `405`,
/// neither counted nor charged (#556). The test
/// `the_open_doors_methods_are_the_routers` pins the two maps together.
pub(crate) const OPEN_DOORS: [(&str, &[Method]); 6] = [
    ("/isAuthenticated", GET_OR_POST),
    ("/waitForAuthentication", GET_OR_POST),
    ("/getHeight", GET_OR_POST),
    ("/getHeaderForHeight", &[Method::POST]),
    ("/getNetwork", GET_OR_POST),
    ("/getVersion", GET_OR_POST),
];

/// The status doors' methods: `GET` (with its `HEAD`) and `POST`.
const GET_OR_POST: &[Method] = &[Method::GET, Method::HEAD, Method::POST];

/// Whether `route` is an open door answering `method`: the call the bound
/// charges and the probe record counts (the gate's question).
pub(crate) fn answers(route: &str, method: &Method) -> bool {
    OPEN_DOORS
        .iter()
        .any(|(door, methods)| *door == route && methods.contains(method))
}

/// The presence doors: one answer per door per originator a minute.
pub const PRESENCE_DOORS: [&str; 2] = ["/isAuthenticated", "/waitForAuthentication"];

/// One bound's figures: one call admitted every `every`, `burst` at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Figure {
    /// The calls admitted at once.
    pub burst: u32,
    /// The emission interval: one call admitted each.
    pub every: Duration,
}

impl Figure {
    /// How far ahead of now the arrival time may run: one interval for
    /// each call of the burst beyond one.
    fn tolerance(self) -> Duration {
        self.every * self.burst.saturating_sub(1)
    }
}

/// The presence doors' figure: one a minute.
pub const PRESENCE: Figure = Figure {
    burst: 1,
    every: Duration::from_secs(60),
};
/// The chain doors' figure: 60 a minute, 10 at once.
pub const CHAIN: Figure = Figure {
    burst: 10,
    every: Duration::from_secs(1),
};
/// The level-0 powers' figure: the chain doors'.
pub const LEVEL_ZERO: Figure = CHAIN;

/// Past this many originators' counters, the idle ones (whose arrival time
/// has passed, the same as none) are dropped before a new one is kept.
const PRUNE_AT: usize = 1024;

/// What a call is bounded as: an open door by its path, or a level-0
/// power at `door`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An open door, by its path: a presence door or a chain door.
    Open,
    /// A protocol power at security level 0.
    LevelZero,
}

/// The bound of one call: its figure, the counter it shares, and the words
/// of its refusal.
struct Bound {
    figure: Figure,
    counter: &'static str,
    window: &'static str,
}

fn bound_of(class: Class, door: &str) -> Bound {
    match class {
        Class::LevelZero => Bound {
            figure: LEVEL_ZERO,
            counter: "level 0",
            window: "the level-0 protocol powers answer 60 a minute per site, 10 at once",
        },
        Class::Open => match PRESENCE_DOORS.iter().find(|d| **d == door) {
            Some(presence) => Bound {
                figure: PRESENCE,
                counter: presence,
                window: "a presence door answers once a minute per site",
            },
            None => Bound {
                figure: CHAIN,
                counter: "chain",
                window: "the chain doors (getHeight, getHeaderForHeight, getNetwork, getVersion) \
                         answer 60 a minute per site, 10 at once",
            },
        },
    }
}

/// The server's counters, shared by every clone: per originator's
/// hostname and counter, the theoretical arrival time of its next call.
#[derive(Clone, Default)]
pub struct OpenBounds(Arc<Counters>);

#[derive(Default)]
struct Counters {
    map: Mutex<HashMap<(String, &'static str), Instant>>,
    /// The clock stood still at this instant ([`OpenBounds::stopped`]);
    /// none reads the time.
    stopped: Option<Instant>,
}

/// A call past its bound: the wait until the next would be admitted.
#[derive(Debug, PartialEq, Eq)]
pub struct Limited {
    /// How long until the next call would be admitted.
    pub wait: Duration,
    /// The bound's words, as the refusal names them.
    pub window: &'static str,
}

impl Limited {
    /// `Retry-After` in whole seconds: the wait rounded up, at least 1.
    #[must_use]
    pub fn retry_after_secs(&self) -> u64 {
        let secs = self.wait.as_secs() + u64::from(self.wait.subsec_nanos() > 0);
        secs.max(1)
    }
}

impl OpenBounds {
    /// Bounds whose clock stands still: no window ever passes, so a
    /// recorded refusal and its `Retry-After` do not depend on how fast the
    /// calls before it ran (the wire harness's worlds). The figures are
    /// the same.
    #[must_use]
    pub fn stopped() -> Self {
        Self(Arc::new(Counters {
            map: Mutex::default(),
            stopped: Some(Instant::now()),
        }))
    }

    /// Admit one call of `host` at `door` as `class` at `now`, or refuse it
    /// with the wait. A refusal moves no counter.
    ///
    /// # Errors
    ///
    /// The call past its bound: [`Limited`], the wait and the window.
    pub fn take(&self, host: &str, door: &str, class: Class, now: Instant) -> Result<(), Limited> {
        self.judge(host, door, class, now, true)
    }

    /// The bound's answer at `now`; with `charge`, an admission moves the
    /// counter, and without it the counter is only read (a level-0 check
    /// before the call's one charge, #556).
    fn judge(
        &self,
        host: &str,
        door: &str,
        class: Class,
        now: Instant,
        charge: bool,
    ) -> Result<(), Limited> {
        let bound = bound_of(class, door);
        let mut map = self.0.map.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (host.to_string(), bound.counter);
        let due = map.get(&key).copied().map_or(now, |t| t.max(now));
        let ahead = due.saturating_duration_since(now);
        let past = ahead.checked_sub(bound.figure.tolerance());
        if let Some(wait) = past.filter(|wait| !wait.is_zero()) {
            return Err(Limited {
                wait,
                window: bound.window,
            });
        }
        if !charge {
            return Ok(());
        }
        if map.len() >= PRUNE_AT && !map.contains_key(&key) {
            map.retain(|_, t| *t > now);
        }
        map.insert(key, due + bound.figure.every);
        Ok(())
    }

    /// The counters kept now (the prune's test).
    #[cfg(test)]
    fn kept(&self) -> usize {
        self.0
            .map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// The bound at the gate's seam (`powers::open`): admit `originator`'s
    /// call at `door`, or the 429 the door serves, logged at warn by name.
    pub(crate) fn check(&self, originator: &str, door: &str, class: Class) -> Result<(), AppError> {
        self.served(originator, door, class, true)
    }

    /// [`check`](Self::check) with the counter unmoved: a level-0 check
    /// read in the call's order, before the one charge (#556).
    pub(crate) fn admits(
        &self,
        originator: &str,
        door: &str,
        class: Class,
    ) -> Result<(), AppError> {
        self.served(originator, door, class, false)
    }

    /// The bound's answer as the door serves it; with `charge`, an
    /// admission moves the counter.
    fn served(
        &self,
        originator: &str,
        door: &str,
        class: Class,
        charge: bool,
    ) -> Result<(), AppError> {
        let host = crate::store::origin_grants::key_of(originator)
            .map_or_else(|_| originator.to_string(), |(host, _)| host);
        let now = self.0.stopped.unwrap_or_else(Instant::now);
        self.judge(&host, door, class, now, charge)
            .map_err(|limited| {
                let secs = limited.retry_after_secs();
                tracing::warn!(originator = %host, door, class = ?class, retry_after = secs, "browser gate: rate limited");
                let what = match class {
                    Class::Open => format!("calls to {door}"),
                    Class::LevelZero => format!("level-0 calls to {door}"),
                };
                AppError::new(
                    Kind::RateLimited,
                    format!(
                        "Too many {what} from this site: {}; retry after {secs} s.",
                        limited.window
                    ),
                )
                .with_retry_after(secs)
            })
    }
}

#[cfg(test)]
#[path = "open_bounds_tests.rs"]
mod served_tests;

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "page.example";

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// Counters for new hosts at `t` until [`PRUNE_AT`] are kept.
    fn fill(bounds: &OpenBounds, t: Instant) {
        for n in bounds.kept()..PRUNE_AT {
            let host = format!("h{n}.example");
            bounds.take(&host, "/getHeight", Class::Open, t).unwrap();
        }
        assert_eq!(bounds.kept(), PRUNE_AT);
    }

    /// Bounds whose chain burst `HOST` spent at the instant returned.
    fn spent_burst() -> (OpenBounds, Instant) {
        let (bounds, t) = (OpenBounds::default(), Instant::now());
        for n in 0..10 {
            assert_eq!(
                bounds.take(HOST, "/getHeight", Class::Open, t),
                Ok(()),
                "{n}"
            );
        }
        (bounds, t)
    }

    /// The chain doors at the burst's edge: the tenth call at once is
    /// admitted, the eleventh refused with a wait of one interval, and the
    /// four doors share the one bound.
    #[test]
    fn the_bursts_last_call_is_admitted_and_the_next_refused() {
        let (bounds, t) = (OpenBounds::default(), Instant::now());
        let doors = [
            "/getHeight",
            "/getHeaderForHeight",
            "/getNetwork",
            "/getVersion",
        ];
        for n in 0..10 {
            assert_eq!(
                bounds.take(HOST, doors[n % 4], Class::Open, t),
                Ok(()),
                "call {n}"
            );
        }
        let refused = bounds.take(HOST, "/getHeight", Class::Open, t).unwrap_err();
        assert_eq!(refused.wait, Duration::from_secs(1));
        assert_eq!(refused.retry_after_secs(), 1);
        assert!(refused.window.contains("60 a minute per site, 10 at once"));
        // A refusal moves no counter: refused again at once, by the same wait.
        let again = bounds
            .take(HOST, "/getVersion", Class::Open, t)
            .unwrap_err();
        assert_eq!(again.wait, Duration::from_secs(1));
    }

    /// The window's reset: one interval after the burst one call is
    /// admitted and the next refused; a minute of quiet refills the burst.
    #[test]
    fn the_window_resets() {
        let (bounds, t) = spent_burst();
        assert!(bounds
            .take(HOST, "/getHeight", Class::Open, at(t, 999))
            .is_err());
        assert_eq!(
            bounds
                .take(HOST, "/getHeight", Class::Open, at(t, 999))
                .unwrap_err()
                .wait,
            Duration::from_millis(1)
        );
        assert_eq!(
            bounds.take(HOST, "/getHeight", Class::Open, at(t, 1000)),
            Ok(())
        );
        assert!(bounds
            .take(HOST, "/getHeight", Class::Open, at(t, 1000))
            .is_err());
        // 60 a minute: one a second sustained is admitted, every one.
        for s in 2..=61 {
            assert_eq!(
                bounds.take(HOST, "/getNetwork", Class::Open, at(t, s * 1000)),
                Ok(()),
                "second {s}"
            );
        }
        let quiet = at(t, 61_000 + 10_000);
        for n in 0..10 {
            assert_eq!(
                bounds.take(HOST, "/getVersion", Class::Open, quiet),
                Ok(()),
                "refilled {n}"
            );
        }
        assert!(bounds
            .take(HOST, "/getVersion", Class::Open, quiet)
            .is_err());
    }

    /// A presence door answers once a minute per door: the second call is
    /// refused with `Retry-After` the minute's rest rounded up, and the
    /// other presence door has its own answer.
    #[test]
    fn a_presence_door_answers_once_a_minute() {
        let (bounds, t) = (OpenBounds::default(), Instant::now());
        assert_eq!(
            bounds.take(HOST, "/isAuthenticated", Class::Open, t),
            Ok(())
        );
        let refused = bounds
            .take(HOST, "/isAuthenticated", Class::Open, at(t, 250))
            .unwrap_err();
        assert_eq!(refused.wait, Duration::from_millis(59_750));
        assert_eq!(refused.retry_after_secs(), 60);
        assert_eq!(
            refused.window,
            "a presence door answers once a minute per site"
        );
        let late = bounds
            .take(HOST, "/isAuthenticated", Class::Open, at(t, 59_000))
            .unwrap_err();
        assert_eq!(late.retry_after_secs(), 1);
        assert_eq!(
            bounds.take(HOST, "/waitForAuthentication", Class::Open, t),
            Ok(())
        );
        assert_eq!(
            bounds.take(HOST, "/isAuthenticated", Class::Open, at(t, 60_000)),
            Ok(())
        );
    }

    /// Per-origin isolation, and the classes apart: one originator past
    /// its chain bound leaves another's untouched and its own level-0 and
    /// presence bounds whole.
    #[test]
    fn each_originator_and_class_has_its_own_bound() {
        let (bounds, t) = spent_burst();
        assert!(bounds.take(HOST, "/getHeight", Class::Open, t).is_err());
        for n in 0..10 {
            assert_eq!(
                bounds.take("other.example", "/getHeight", Class::Open, t),
                Ok(()),
                "{n}"
            );
            assert_eq!(
                bounds.take(HOST, "/createSignature", Class::LevelZero, t),
                Ok(()),
                "{n}"
            );
        }
        let level_zero = bounds
            .take(HOST, "/encrypt", Class::LevelZero, t)
            .unwrap_err();
        assert!(level_zero.window.starts_with("the level-0 protocol powers"));
        assert_eq!(
            bounds.take(HOST, "/isAuthenticated", Class::Open, t),
            Ok(())
        );
    }

    /// A read without the charge moves no counter (#556): with one call of
    /// the burst left, any number of reads admit, the take after them is
    /// the burst's last, and a read past it refuses with the take's wait.
    #[test]
    fn a_read_without_the_charge_moves_no_counter() {
        let read =
            |bounds: &OpenBounds, t| bounds.judge(HOST, "/encrypt", Class::LevelZero, t, false);
        let (bounds, t) = (OpenBounds::default(), Instant::now());
        for n in 0..9 {
            bounds.take(HOST, "/encrypt", Class::LevelZero, t).unwrap();
            assert_eq!(read(&bounds, t), Ok(()), "{n}");
        }
        for _ in 0..5 {
            assert_eq!(read(&bounds, t), Ok(()));
        }
        assert_eq!(bounds.take(HOST, "/encrypt", Class::LevelZero, t), Ok(()));
        let past = read(&bounds, t).map_err(|l| l.wait);
        assert_eq!(past, Err(Duration::from_secs(1)));
        assert!(bounds.take(HOST, "/encrypt", Class::LevelZero, t).is_err());
    }

    /// `Retry-After` against the wait: whole seconds rounded up, never 0.
    #[test]
    fn retry_after_is_the_wait_rounded_up() {
        for (ms, secs) in [
            (1, 1),
            (999, 1),
            (1000, 1),
            (1001, 2),
            (59_750, 60),
            (60_000, 60),
        ] {
            let limited = Limited {
                wait: Duration::from_millis(ms),
                window: "",
            };
            assert_eq!(limited.retry_after_secs(), secs, "{ms} ms");
        }
        let none = Limited {
            wait: Duration::ZERO,
            window: "",
        };
        assert_eq!(none.retry_after_secs(), 1);
    }

    /// A stopped clock never refills: the burst's eleventh call is refused
    /// however late it comes, with the full interval's wait.
    #[tokio::test]
    async fn a_stopped_clock_never_refills() {
        let bounds = OpenBounds::stopped();
        for n in 0..10 {
            assert!(
                bounds.check(HOST, "/getNetwork", Class::Open).is_ok(),
                "{n}"
            );
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let refused = bounds.check(HOST, "/getHeight", Class::Open).unwrap_err();
        let text = format!("{refused:?}");
        assert!(
            text.contains("RateLimited") && text.contains("retry after 1 s."),
            "{text}"
        );
    }

    /// The prune's edges: under [`PRUNE_AT`] nothing is dropped, however
    /// idle; at it, a call of a kept counter drops nothing, and a counter
    /// whose arrival time is now exactly is idle (the same as none).
    #[test]
    fn the_prune_runs_only_for_a_new_counter_at_the_cap() {
        let (bounds, t) = (OpenBounds::default(), Instant::now());
        bounds
            .take("idle.example", "/getHeight", Class::Open, t)
            .unwrap();
        bounds
            .take(HOST, "/getHeight", Class::Open, at(t, 5_000))
            .unwrap();
        assert_eq!(bounds.kept(), 2, "under the cap the idle counter stays");
        fill(&bounds, t);
        bounds
            .take(HOST, "/getHeight", Class::Open, at(t, 5_000))
            .unwrap();
        assert_eq!(
            bounds.kept(),
            PRUNE_AT,
            "a kept counter's call drops nothing"
        );
        // Every counter but HOST's is due at t + 1 s: at that instant they
        // are idle, and a new counter drops them.
        bounds
            .take("new.example", "/getHeight", Class::Open, at(t, 1_000))
            .unwrap();
        assert_eq!(bounds.kept(), 2, "HOST's and the new one");
    }

    /// The counters stay bounded: past [`PRUNE_AT`] the idle ones go, and
    /// a busy one stays.
    #[test]
    fn idle_counters_are_pruned() {
        let (bounds, t) = spent_burst();
        fill(&bounds, t);
        let later = at(t, 5_000);
        bounds
            .take("new.example", "/getHeight", Class::Open, later)
            .unwrap();
        assert_eq!(bounds.kept(), 2, "the busy counter and the new one");
        // The busy counter kept its place: five seconds on, five calls of
        // the burst are back, not ten.
        for n in 0..5 {
            assert_eq!(
                bounds.take(HOST, "/getHeight", Class::Open, later),
                Ok(()),
                "{n}"
            );
        }
        assert!(bounds.take(HOST, "/getHeight", Class::Open, later).is_err());
    }
}
