//! Test-only tracing setup. Not part of the supported API.
//!
//! # Why a permanently registered global subscriber is required (#356)
//!
//! `tracing` caches each callsite's [`Interest`] process-wide the first time that callsite is
//! reached. How the verdict is computed depends on how many dispatchers are registered
//! (`tracing-core` 0.1.36, `callsite.rs`):
//!
//! * **At most one registered** — `Dispatchers::has_just_one` is set, and `Rebuilder::JustOne`
//!   asks `dispatcher::get_default`: the **registering thread's own** current default, not the
//!   global one and not the set of registered dispatchers.
//! * **Two or more registered** — every live registered dispatcher is asked, and the verdicts are
//!   folded with `Interest::and`, which yields `sometimes` whenever two of them disagree.
//!
//! A test binary installs no global subscriber, so a thread that is *not* inside
//! [`tracing::subscriber::with_default`] has `NoSubscriber` as its default, and that answers
//! `Interest::never`. If the only dispatcher ever registered is one test's scoped subscriber, then
//! `has_just_one` stays true, and an unsubscribed thread that reaches the callsite first gets its
//! `never` cached **for the whole process**. The capturing test on another thread then observes
//! nothing — even though its subscriber is live, its level filter permits the event, and
//! `event_enabled!` on a *different* callsite in the same closure reports `true`.
//!
//! Under `cargo test` that is a race against every other test exercising the same code path, and it
//! surfaces on small CI runners far more readily than on a many-core host.
//!
//! [`init`] closes it by installing a global default that is **never dropped**, so at least one
//! dispatcher is always registered:
//!
//! * with a scoped subscriber also active there are two, `has_just_one` is false, and the scoped
//!   subscriber is therefore consulted at registration instead of being invisible;
//! * with no scoped subscriber, `get_default` returns this subscriber rather than `NoSubscriber`,
//!   so the cached verdict is still not `never`.
//!
//! [`Interest`]: tracing::subscriber::Interest

use tracing::level_filters::LevelFilter;
use tracing::span;
use tracing::subscriber::Interest;
use tracing::{Event, Id, Metadata, Subscriber};

/// Records nothing; exists only to keep callsite interest permissive process-wide.
struct InterestFloor;

impl Subscriber for InterestFloor {
    /// `sometimes`, never `always`: an `always` verdict would let events bypass the current
    /// subscriber's `enabled()` and defeat each test's own level filter.
    fn register_callsite(&self, _: &Metadata<'_>) -> Interest {
        Interest::sometimes()
    }

    /// Keeps the global max level at `TRACE`, so a `debug!`/`trace!` callsite is not short-circuited
    /// by the level check that runs before the interest lookup.
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }

    /// This subscriber is never itself interested, so a thread with no capturing subscriber of its
    /// own still emits nothing.
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Install the process-wide interest floor. Idempotent and safe from any thread.
///
/// Every test that captures events through [`tracing::subscriber::with_default`] must run after
/// this. In-crate tests get it for free — `test_trace::capture` calls it — so this is public only so
/// that tests in `tests/` can reach it, and it is not part of the supported API.
pub fn init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Errors only if a global default is already installed, which is equally fine.
        let _ = tracing::subscriber::set_global_default(InterestFloor);
    });
}
