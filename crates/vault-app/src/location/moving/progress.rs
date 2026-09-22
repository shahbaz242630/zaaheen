//! What a running move has done so far, for a screen to show while it runs
//! (ADR-105 amendment 1, L-f, `VAULT-KEY-AND-LOCATION.md`).
//!
//! Only [`DiskFs`](super::fs::DiskFs) reports: the phase as each step of the
//! move begins, and every chunk it copies (M3) or reads back (M4). The
//! in-memory crash model reports nothing, so the crash enumeration is the
//! same with or without a screen. Nothing here decides anything: a move with
//! a progress sink ends exactly as one without.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Where a move is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MovePhase {
    /// M1: waiting to have the memories to itself (an AI app letting go, a
    /// window finishing closing).
    #[default]
    Waiting,
    /// M3: copying; `done` of `total` bytes.
    Copying,
    /// M4: reading every copied file back; `done` of `total` bytes.
    Checking,
    /// M5–M6: switching the record and removing the old copy.
    Finishing,
}

/// One consistent reading of a move's progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveSnapshot {
    pub phase: MovePhase,
    /// Bytes handled so far in this phase; never more than `total`.
    pub done: u64,
    /// Bytes this phase handles in all (0 for the phases that count none).
    pub total: u64,
}

/// A running move's progress, shared between the move and whoever shows it.
/// One lock, so a reader never sees one phase's count under another's name.
#[derive(Debug, Default)]
pub struct MoveProgress {
    now: Mutex<MoveSnapshot>,
    /// Every phase change, for the tests.
    #[cfg(test)]
    changes: Mutex<Vec<Change>>,
    /// Every byte this phase was told of, not held to its total: the
    /// reading's cap would hide a chunk counted twice.
    #[cfg(test)]
    told: Mutex<u64>,
}

/// A phase change, as a test sees it.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Change {
    /// The reading as the old phase ended.
    pub(crate) ended: MoveSnapshot,
    /// Every byte the old phase was told of, before any cap.
    pub(crate) told: u64,
    /// The phase that began, and its total.
    pub(crate) began: MovePhase,
    pub(crate) total: u64,
}

impl MoveProgress {
    /// Where the move is now.
    pub fn snapshot(&self) -> MoveSnapshot {
        *self.lock()
    }

    /// A phase begins, handling `total` bytes; its count starts at zero.
    pub(crate) fn begin(&self, phase: MovePhase, total: u64) {
        let mut now = self.lock();
        #[cfg(test)]
        {
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            self.changes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Change {
                    ended: *now,
                    told: *told,
                    began: phase,
                    total,
                });
            *told = 0;
        }
        *now = MoveSnapshot {
            phase,
            done: 0,
            total,
        };
    }

    /// `bytes` more handled in this phase, never counted past its total.
    pub(crate) fn advance(&self, bytes: u64) {
        let mut now = self.lock();
        now.done = now.done.saturating_add(bytes).min(now.total);
        #[cfg(test)]
        {
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            *told = told.saturating_add(bytes);
        }
    }

    #[cfg(test)]
    pub(crate) fn changes(&self) -> Vec<Change> {
        self.changes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A reading is plain numbers: a panic elsewhere while holding the lock
    /// leaves nothing half-written that matters, so a poisoned lock is read.
    fn lock(&self) -> MutexGuard<'_, MoveSnapshot> {
        self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
