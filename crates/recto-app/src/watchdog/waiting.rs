//! The watcher's next wait, on its own stack: no timer survives an observed idle.

use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Park,
    Wait(Duration),
    Sample,
}

#[derive(Default)]
pub(super) struct Waiting {
    due: Option<Duration>,
}

impl Waiting {
    pub(super) fn next(&mut self, busy: bool, now: Duration) -> Action {
        if !busy {
            self.due = None;
            return Action::Park;
        }
        let due = *self.due.get_or_insert(now + super::LOOK_EVERY);
        if now < due {
            Action::Wait(due - now)
        } else {
            self.due = None;
            Action::Sample
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a brief main-thread pass leaves a timed wake and Mach sample after it has gone idle.
    #[test]
    fn an_idle_watch_cancels_its_pending_sample() {
        let mut wait = Waiting::default();
        let ms = Duration::from_millis;
        assert_eq!(wait.next(false, ms(0)), Action::Park);
        assert_eq!(wait.next(true, ms(10)), Action::Wait(ms(250)));
        assert_eq!(wait.next(false, ms(20)), Action::Park);
        assert_eq!(wait.next(false, ms(260)), Action::Park);
        assert_eq!(wait.next(true, ms(300)), Action::Wait(ms(250)));
        assert_eq!(wait.next(true, ms(550)), Action::Sample);
        assert_eq!(wait.next(true, ms(600)), Action::Wait(ms(250)));
        assert_eq!(wait.next(false, ms(900)), Action::Park, "an already-due look is cancelled too");
    }

    /// Failure: repeated busy notifications postpone a real stall's looks or cause a burst of samples.
    #[test]
    fn a_busy_watch_keeps_its_sample_deadline() {
        let mut wait = Waiting::default();
        let ms = Duration::from_millis;
        assert_eq!(wait.next(true, ms(0)), Action::Wait(ms(250)));
        for now in [1, 50, 249] {
            assert_eq!(wait.next(true, ms(now)), Action::Wait(ms(250 - now)));
        }
        assert_eq!(wait.next(true, ms(250)), Action::Sample);
        assert_eq!(wait.next(true, ms(251)), Action::Wait(ms(250)));
        assert_eq!(wait.next(true, ms(400)), Action::Wait(ms(101)));
        assert_eq!(wait.next(true, ms(501)), Action::Sample);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                size_of::<crate::watchdog::Shared>(),
                1520,
                "the whole shared watch includes the fixed startup trace, with no growth for its wait state"
            );
            assert_eq!(size_of::<Waiting>(), 16, "one optional deadline, on the watcher stack");
        }
    }
}
