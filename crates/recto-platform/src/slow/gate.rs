//! **Seams held by a test in this process, and let through one arrival at a time, in the order the test says** (step R1.4, the headless kit's). Where the slow
//! mode makes a seam late by a time, which a loaded machine can beat or miss, a gate makes it late by an order: what arrives at a held seam waits there, a
//! [`super::pause`] on its own thread and a [`super::later`] call kept, until the test lets it through ([`let_through`], [`open`]). So a test of "the state comes
//! in after the window's up, and sleep files are written after the quit's answer" says that order and gets it, with no sleep and no timing.
//!
//! For tests only: nothing in the app holds a seam, and a release build never does ([`super::RELEASE`]). It's the process's, as the slow mode is: a test that
//! holds a seam runs in a process of its own, as nextest runs every test. Unheld, each seam costs one load of an atomic. A held arrival waits at most
//! [`super::HOLD_LIMIT`], so a test that never lets go fails rather than hangs.

use super::{Call, HOLD_LIMIT, RELEASE, Seam};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

/// Whether any seam is held: what an unheld seam looks at, and no more.
static ANY: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Held {
    /// For each seam: held or not, how many have arrived, how many of those may go, and the calls kept for it.
    seams: [SeamGate; Seam::ALL.len()],
}

#[derive(Default)]
struct SeamGate {
    held: bool,
    arrived: u64,
    let_through: u64,
    kept: VecDeque<Call>,
}

struct Gates {
    held: Mutex<Held>,
    moved: Condvar,
}

fn gates() -> &'static Gates {
    static GATES: OnceLock<Gates> = OnceLock::new();
    GATES.get_or_init(|| Gates {
        held: Mutex::new(Held::default()),
        moved: Condvar::new(),
    })
}

fn held() -> MutexGuard<'static, Held> {
    gates().held.lock().unwrap_or_else(PoisonError::into_inner)
}

/// **Holds these seams,** from now on, until each is [`open`]ed. What arrives is counted from here, for a seam that wasn't held already.
pub fn hold(seams: &[Seam]) {
    if RELEASE {
        return;
    }
    let mut held = held();
    for seam in seams {
        let gate = &mut held.seams[*seam as usize];
        if !gate.held {
            gate.held = true;
            gate.arrived = 0;
            gate.let_through = 0;
        }
    }
    ANY.store(true, Ordering::SeqCst);
}

/// Whether anything's held, for what decides by it ([`super::is_on`]).
pub(super) fn any() -> bool {
    ANY.load(Ordering::Relaxed)
}

/// How many have arrived at a seam since it was held, waiting or let through.
pub fn arrived(seam: Seam) -> u64 {
    held().seams[seam as usize].arrived
}

/// How many are waiting at a seam now.
pub fn waiting(seam: Seam) -> u64 {
    let held = held();
    let gate = &held.seams[seam as usize];
    gate.arrived.saturating_sub(gate.let_through)
}

/// **Lets the next `n` arrivals at a seam through,** in the order they came: those waiting go, and if fewer are, the next to come go by. A kept call is made
/// here, on the caller's thread, once the gate's lock is let go of.
pub fn let_through(seam: Seam, n: u64) {
    let calls: Vec<Call> = {
        let mut held = held();
        let gate = &mut held.seams[seam as usize];
        let before = gate.let_through;
        gate.let_through = gate.let_through.saturating_add(n);
        // Kept calls are the arrivals, in order, that haven't gone: as many as now may.
        let going = usize::try_from(gate.let_through.min(gate.arrived).saturating_sub(before)).unwrap_or(usize::MAX);
        let going = going.min(gate.kept.len());
        gate.kept.drain(..going).collect()
    };
    gates().moved.notify_all();
    for call in calls {
        call();
    }
}

/// **A seam no longer held:** everything waiting goes, kept calls are made here, and what comes later goes by.
pub fn open(seam: Seam) {
    let calls: Vec<Call> = {
        let mut held = held();
        let gate = &mut held.seams[seam as usize];
        gate.held = false;
        gate.let_through = gate.arrived;
        gate.kept.drain(..).collect()
    };
    gates().moved.notify_all();
    for call in calls {
        call();
    }
}

/// Every seam opened: what a process's end comes to.
pub fn open_all() {
    if !any() {
        return;
    }
    for seam in Seam::ALL {
        open(seam);
    }
}

/// **At a seam, on a thread that can wait:** if it's held, waits its turn. Says whether it was held, so the slow mode's own delay is taken only when it wasn't.
pub(super) fn pause(seam: Seam) -> bool {
    if !any() {
        return false;
    }
    let gates = gates();
    let mut held = held();
    if !held.seams[seam as usize].held {
        return false;
    }
    let gate = &mut held.seams[seam as usize];
    gate.arrived += 1;
    let mine = gate.arrived;
    let began = Instant::now();
    loop {
        let gate = &held.seams[seam as usize];
        if !gate.held || gate.let_through >= mine {
            return true;
        }
        let left = HOLD_LIMIT.saturating_sub(began.elapsed());
        if left.is_zero() {
            return true;
        }
        held = gates.moved.wait_timeout(held, left).unwrap_or_else(PoisonError::into_inner).0;
    }
}

/// **A call to the host at a seam:** if it's held, kept until its turn, and given back as `Err` when it may go now.
pub(super) fn later(seam: Seam, call: Call) -> Result<(), Call> {
    if !any() {
        return Err(call);
    }
    let mut held = held();
    let gate = &mut held.seams[seam as usize];
    if !gate.held {
        return Err(call);
    }
    gate.arrived += 1;
    if gate.let_through >= gate.arrived {
        return Err(call);
    }
    gate.kept.push_back(call);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// Failure: a held seam lets something through before its turn, out of the order it came in, or never; or a call kept at it is made twice, early or not at
    /// all. Two threads wait at a held seam and two calls are kept: nothing goes until let through, then one at a time in the order they came, and opening lets
    /// the rest and all that come later go.
    #[test]
    fn a_held_seam_lets_through_in_the_order_things_came_and_only_when_told() {
        hold(&[Seam::SleepWrite, Seam::Remake]);
        let (went, order) = channel();
        for n in 0..2u64 {
            let went = went.clone();
            std::thread::spawn(move || {
                super::super::pause(Seam::SleepWrite);
                went.send(n).unwrap();
            });
            // The second only once the first is waiting, so the order they came in is known.
            let began = Instant::now();
            while waiting(Seam::SleepWrite) < n + 1 {
                assert!(began.elapsed() < Duration::from_secs(10), "never arrived");
                std::thread::yield_now();
            }
        }
        assert!(order.recv_timeout(Duration::from_millis(50)).is_err(), "nothing goes while held");
        let_through(Seam::SleepWrite, 1);
        assert_eq!(order.recv_timeout(Duration::from_secs(10)), Ok(0), "the first that came");
        assert!(order.recv_timeout(Duration::from_millis(50)).is_err(), "and only it");
        let calls = Arc::new(AtomicU64::new(0));
        for _ in 0..2 {
            let calls = Arc::clone(&calls);
            super::super::later(Seam::Remake, move || {
                calls.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0, "kept");
        let_through(Seam::Remake, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one made, on this thread");
        open(Seam::SleepWrite);
        open(Seam::Remake);
        assert_eq!(order.recv_timeout(Duration::from_secs(10)), Ok(1), "opened, the rest go");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "and the kept call is made, once");
        super::super::pause(Seam::SleepWrite);
        assert_eq!(waiting(Seam::SleepWrite), 0, "what comes after goes by");
    }
}
