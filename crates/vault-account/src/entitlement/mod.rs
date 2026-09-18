//! The time model: whether a verified lease entitles this computer right
//! now, and the local record (`state.json`) that keeps that honest when the
//! clock is wrong (§8.26 §4).
//!
//! Everything here is a pure function of the lease, the local record and the
//! current system time, passed in as epoch seconds. Reading and writing the
//! files, the lock and the refresh itself are S1 step 5.
//!
//! # The entitlement check (§8.26 §4, quoted)
//!
//! > `floor = max(stored_floor, system_now)`, where `stored_floor` =
//! > `state.floor` **only if `state.lease_issued_at == lease.issued_at`**,
//! > else `client_time` (a reader that sees a new lease before `state.json`
//! > is rewritten must not use the old floor).
//! > `elapsed = max(0, floor − client_time)`.
//! >
//! > Entitled iff `elapsed < offline_days` and:
//! > - `trial`: `elapsed < trial_ends_at − issued_at`;
//! > - `active` / `payment_failed`: `elapsed < active_until − issued_at`.
//! >
//! > `system_now < client_time − 10 min` (clock moved backwards past the
//! > anchor) ⇒ treated as "needs refresh"; offline ⇒ not entitled
//!
//! All maths is relative to the lease's own `client_time`, so a clock that
//! is simply wrong (fast or slow) changes nothing; only a clock that
//! **moves** matters, and the floor stops it moving backwards.
//!
//! # The local record (§8.26 §4, quoted)
//!
//! > `last_active_anchor` and `last_refresh_attempt` always merge with
//! > `max()`, across leases too. At sign-in, `last_active_anchor =
//! > issued_at` of the first lease.
//! > **Only `floor` is lease-scoped.** A new lease resets `floor` to its
//! > `client_time`. For the same `lease_issued_at`, `floor` merges with
//! > `max()`. A writer holding an older `lease_issued_at` never changes
//! > `floor`.
//! > Deleting the file only lowers the floor to "now".
//!
//! A value of `0` (or less) in the record means "absent": a missing or
//! damaged file must never sign anyone out.
//!
//! # 30 days unused (§8.26 §4, quoted)
//!
//! > activity is recorded in **server time** (`last_active_anchor = issued_at
//! > + elapsed`, at most hourly), so the client's clock error never enters
//! > it. The rule is evaluated **only after a successful refresh**, as
//! > `server issued_at − last_active_anchor > 30 days` → sign out
//!
//! # Every denial is refresh-then-decide
//!
//! A [`Denial`] never means "locked" by itself: the caller refreshes first
//! (§8.26 §4) and decides from the new lease, or, if the refresh fails, from
//! this one. Only [`Denial::Ended`] is the server's own word; after a failed
//! refresh every other denial reads "Zaaheen couldn't confirm your
//! subscription".

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::lease::{Lease, LeaseState};

/// One day in seconds.
pub const DAY: i64 = 86_400;

/// How far the clock may fall behind the lease's `client_time` before the
/// lease stops being trusted (§8.26 §4: 10 minutes).
pub const CLOCK_BEHIND_TOLERANCE: i64 = 10 * 60;

/// Skew between the client's clock and the server's at receipt beyond which
/// the desktop shows "Your computer's clock is wrong" (§8.26 §4: 24 hours).
pub const CLOCK_WARNING_SKEW: i64 = DAY;

/// Unused for longer than this, and the next successful refresh signs out
/// (§8.26 §4, founder-locked: 30 days).
pub const UNUSED_SIGN_OUT_AFTER: i64 = 30 * DAY;

/// Activity is recorded at most this often (§8.26 §4: hourly).
pub const ACTIVITY_GRANULARITY: i64 = 60 * 60;

/// At most one refresh attempt per minute (§8.26 §4).
pub const REFRESH_MIN_INTERVAL: i64 = 60;

/// After a signed `ended`, refresh-on-denial backs off to hourly (§8.26 §4).
pub const REFRESH_INTERVAL_AFTER_ENDED: i64 = 60 * 60;

/// At keeper start and desktop open, refresh a lease older than this
/// (§8.26 §4: 24 hours) ...
pub const REFRESH_WHEN_OLDER_THAN: i64 = DAY;

/// ... or one whose deadline is this close (§8.26 §4: 3 days).
pub const REFRESH_WHEN_DEADLINE_WITHIN: i64 = 3 * DAY;

/// Why a lease does not entitle this computer right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    /// The server signed `ended`. `was_paid` is true when the lease names a
    /// paid period, which picks "subscription ended" over "trial ended".
    Ended {
        /// Whether the account ever had a paid period.
        was_paid: bool,
    },
    /// The trial end or the paid period end has passed.
    DeadlinePassed,
    /// The lease has been used offline for its whole allowance.
    OfflineTooLong,
    /// The clock is more than 10 minutes behind the lease's anchor.
    ClockBehind,
}

/// The verdict on a lease at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entitlement {
    /// Entitled. `payment_failed` asks for the "update your card" banner.
    Entitled {
        /// The server signed `payment_failed`.
        payment_failed: bool,
    },
    /// Not entitled (refresh first, then decide).
    Denied(Denial),
}

/// [`assess`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    /// The verdict.
    pub entitlement: Entitlement,
    /// Local seconds since the lease was received, never negative and never
    /// shrinking while the lease is the same (the floor).
    pub elapsed: i64,
    /// Seconds of entitlement left, the smaller of the state's deadline and
    /// the offline allowance; 0 when denied. For trial banners (day 23, last
    /// 5 days).
    pub remaining: i64,
}

/// The local record, `state.json` (§8.26 §4). Written under the lock by the
/// keeper or the desktop only, never a relay. Unknown fields are ignored and
/// missing ones read as 0 ("absent").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalState {
    /// `issued_at` of the lease `floor` belongs to.
    pub lease_issued_at: i64,
    /// The highest system time seen while that lease was current.
    pub floor: i64,
    /// Last activity, in server time.
    pub last_active_anchor: i64,
    /// Last refresh attempt, in system time.
    pub last_refresh_attempt: i64,
}

impl LocalState {
    /// Combine the record on disk with the one this process wants to write,
    /// by the §8.26 §4 rules (see the module docs).
    pub fn merge(disk: &LocalState, mine: &LocalState) -> LocalState {
        use std::cmp::Ordering;
        let (lease_issued_at, floor) = match mine.lease_issued_at.cmp(&disk.lease_issued_at) {
            Ordering::Greater => (mine.lease_issued_at, mine.floor),
            Ordering::Equal => (mine.lease_issued_at, mine.floor.max(disk.floor)),
            Ordering::Less => (disk.lease_issued_at, disk.floor),
        };
        LocalState {
            lease_issued_at,
            floor,
            last_active_anchor: mine.last_active_anchor.max(disk.last_active_anchor),
            last_refresh_attempt: mine.last_refresh_attempt.max(disk.last_refresh_attempt),
        }
    }

    /// The record after a new lease is received: `floor` resets to the
    /// lease's `client_time`; at sign-in (no activity yet) the activity
    /// anchor starts at the lease's `issued_at`.
    pub fn on_new_lease(&self, lease: &Lease) -> LocalState {
        let last_active_anchor = if self.last_active_anchor > 0 {
            self.last_active_anchor
        } else {
            lease.issued_at()
        };
        LocalState {
            lease_issued_at: lease.issued_at(),
            floor: lease.client_time(),
            last_active_anchor,
            last_refresh_attempt: self.last_refresh_attempt,
        }
    }

    /// The record with the floor raised to `now` for `lease` (to persist, at
    /// most once a minute, while `lease` is current). A lease the record does
    /// not know starts afresh from its own `client_time`.
    pub fn with_floor(&self, lease: &Lease, now: i64) -> LocalState {
        LocalState {
            lease_issued_at: lease.issued_at(),
            floor: stored_floor(lease, self).max(now),
            ..*self
        }
    }

    /// The record after a refresh attempt at `now`.
    pub fn with_refresh_attempt(&self, now: i64) -> LocalState {
        LocalState {
            last_refresh_attempt: self.last_refresh_attempt.max(now),
            ..*self
        }
    }

    /// The record after activity (a served call, the desktop opening), or
    /// `None` when nothing needs writing: denied, or less than an hour since
    /// the last recorded activity.
    pub fn with_activity(&self, lease: &Lease, assessment: &Assessment) -> Option<LocalState> {
        if !matches!(assessment.entitlement, Entitlement::Entitled { .. }) {
            return None;
        }
        // Server time: the lease's issue time plus local elapsed, never the
        // local clock itself.
        let anchor = lease.issued_at().saturating_add(assessment.elapsed);
        if anchor.saturating_sub(self.last_active_anchor) < ACTIVITY_GRANULARITY {
            return None;
        }
        Some(LocalState {
            last_active_anchor: anchor,
            ..*self
        })
    }
}

/// The floor to start from for `lease`: the record's, if it belongs to this
/// lease, else the lease's own `client_time`.
fn stored_floor(lease: &Lease, state: &LocalState) -> i64 {
    if state.lease_issued_at == lease.issued_at() {
        state.floor
    } else {
        lease.client_time()
    }
}

/// Judge `lease` at system time `now`, given the local record.
pub fn assess(lease: &Lease, state: &LocalState, now: i64) -> Assessment {
    let client_time = lease.client_time();
    let floor = stored_floor(lease, state).max(now);
    let elapsed = floor.saturating_sub(client_time).max(0);
    let denied = |denial| Assessment {
        entitlement: Entitlement::Denied(denial),
        elapsed,
        remaining: 0,
    };

    if now < client_time.saturating_sub(CLOCK_BEHIND_TOLERANCE) {
        return denied(Denial::ClockBehind);
    }
    // Each state's own deadline, as a duration from issue (all relative to
    // the lease, so the client's clock error cancels out).
    let deadline = |at: Option<i64>| at.map(|at| at.saturating_sub(lease.issued_at()));
    let (term, payment_failed) = match lease.state() {
        LeaseState::Ended => {
            return denied(Denial::Ended {
                was_paid: lease.active_until().is_some(),
            })
        }
        LeaseState::Trial => (deadline(lease.trial_ends_at()), false),
        LeaseState::Active => (deadline(lease.active_until()), false),
        LeaseState::PaymentFailed => (deadline(lease.active_until()), true),
    };
    // verify() guarantees the deadline for these states; absent reads as
    // already passed rather than as unlimited (SP-4).
    let term_left = term.unwrap_or(0).saturating_sub(elapsed);
    if term_left <= 0 {
        return denied(Denial::DeadlinePassed);
    }
    let offline_left = i64::from(lease.offline_days())
        .saturating_mul(DAY)
        .saturating_sub(elapsed);
    if offline_left <= 0 {
        return denied(Denial::OfflineTooLong);
    }
    Assessment {
        entitlement: Entitlement::Entitled { payment_failed },
        elapsed,
        remaining: term_left.min(offline_left),
    }
}

/// At receipt: is the computer's clock more than a day off the server's?
/// Information only; entitlement is unaffected (§8.26 §4).
pub fn clock_looks_wrong(lease: &Lease) -> bool {
    lease
        .client_time()
        .saturating_sub(lease.issued_at())
        .saturating_abs()
        > CLOCK_WARNING_SKEW
}

/// May a refresh be attempted at `now`? At most one a minute, hourly after a
/// signed `ended`; a recorded attempt later than `now` counts as none (a
/// clock that jumped back must not block refreshes).
pub fn refresh_allowed(state: &LocalState, now: i64, after_ended: bool) -> bool {
    let last = state.last_refresh_attempt;
    if last <= 0 || last > now {
        return true;
    }
    let interval = if after_ended {
        REFRESH_INTERVAL_AFTER_ENDED
    } else {
        REFRESH_MIN_INTERVAL
    };
    now.saturating_sub(last) >= interval
}

/// At keeper start and desktop open: refresh when the lease is more than a
/// day old or its deadline is within three days.
pub fn stale_at_start(assessment: &Assessment) -> bool {
    assessment.elapsed >= REFRESH_WHEN_OLDER_THAN
        || assessment.remaining < REFRESH_WHEN_DEADLINE_WITHIN
}

/// After a successful refresh: has this computer been unused for more than
/// 30 days, measured in server time? `fresh` is the lease just received.
/// A missing or damaged record (anchor 0 or less) never signs anyone out.
pub fn unused_too_long(state: &LocalState, fresh: &Lease) -> bool {
    state.last_active_anchor > 0
        && fresh.issued_at().saturating_sub(state.last_active_anchor) > UNUSED_SIGN_OUT_AFTER
}
