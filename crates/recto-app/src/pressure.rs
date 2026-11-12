//! **Memory pressure: acted on, or only counted.** The system saying memory's short puts every tab off screen to sleep at once (`recto-ffi`'s sleeper). A quiet run, the app under
//! test, hears it only from `qa memory-pressure`: the machine's own pressure, from other work on it, is counted there and nothing is done about it, so a test's tabs stay as the test
//! put them, and `qa state` still says the machine was under pressure while it ran. Outside a quiet run the system's pressure is acted on, as it always was.

/// Where word of memory pressure came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The system's dispatch source, which the host keeps.
    System,
    /// `qa memory-pressure warning` or `critical`, a test standing in for the system.
    Qa,
}

/// What's done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// The tabs off screen go to sleep, and the page views let go of what they can.
    Act,
    /// Counted for `qa state`, and nothing else.
    Count,
}

/// What's done with memory pressure from `source`, in a quiet run (`qa_mode`) or not.
#[must_use]
pub fn heard(qa_mode: bool, source: Source) -> Heard {
    match (qa_mode, source) {
        (true, Source::System) => Heard::Count,
        (false, Source::System) | (_, Source::Qa) => Heard::Act,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a quiet run acts on the machine's own memory pressure, so a test's tabs go to sleep under it when other work loads the Mac (full check, 2026-11-12: `pressure_and_the_budget_do_what_being_left_alone_does` saw 2 documents held of 3, during the kernel's only warning that hour); or the app a person uses stops acting on it; or a test's own `qa memory-pressure` does nothing.
    #[test]
    fn a_quiet_run_counts_the_systems_pressure_and_acts_on_its_own() {
        assert_eq!(heard(true, Source::System), Heard::Count);
        assert_eq!(heard(false, Source::System), Heard::Act, "the app a person uses");
        assert_eq!(heard(true, Source::Qa), Heard::Act, "a test's own");
    }
}
