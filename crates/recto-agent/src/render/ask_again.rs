//! **Asking again while the Mac is too busy,** for the exports that ask the render service once (`export svg`, an image, the list of a page's images). A busy answer ([`RenderFailure::is_busy`]) says nothing of the page and the service remembers nothing of it, so the person isn't handed "too busy, it will be tried again" with nothing trying: the export starts its output over and asks again. At most `2 * MAX_BUSY` asks (one contents ask can be busy from three separate runs of the service, a start's, an open's and a page read's, each bounded by [`MAX_BUSY`] on its own; as `agreement::asked` and the indexer's), and none after the command's `deadline`; the last busy answer is then the answer. The same idea as `agreement::asked` and the indexer's `asked`, which count starts and opens apart and could take this one.

use recto_render_service::{MAX_BUSY, RenderFailure};
use std::time::{Duration, Instant};

/// Runs `ask` with the run's `state` and the time left to `deadline` (each ask gets only that, so a clip's calls and the asks again share one deadline); on a busy answer with asks and time left, `start_over` clears whatever the failed ask left in `state` and in the output (a failure of it ends the export as that failure), and it is asked again.
pub(crate) fn asked_again<S, T>(
    deadline: Instant,
    state: &mut S,
    mut ask: impl FnMut(&mut S, Duration) -> Result<T, RenderFailure>,
    mut start_over: impl FnMut(&mut S) -> Result<(), RenderFailure>,
) -> Result<T, RenderFailure> {
    let mut asks = 0u8;
    loop {
        asks += 1;
        match ask(state, deadline.saturating_duration_since(Instant::now())) {
            Err(busy) if busy.is_busy() && asks < 2 * MAX_BUSY && Instant::now() < deadline => start_over(state)?,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// Failure: a busy answer ends an export that a second ask would have finished, or the output isn't started over so the second ask's bytes follow the first's.
    #[test]
    fn a_busy_answer_starts_the_output_over_and_asks_again() {
        let mut out: Vec<u8> = Vec::new();
        let mut busy_left = 1;
        let got = asked_again(
            far(),
            &mut out,
            |out, _| {
                out.push(1);
                if busy_left > 0 {
                    busy_left -= 1;
                    Err(RenderFailure::TooBusy { page: 0 })
                } else {
                    Ok(out.len())
                }
            },
            |out| {
                out.clear();
                Ok(())
            },
        );
        assert_eq!(got, Ok(1), "only the second ask's byte is in the output");
    }

    /// Failure: a page whose worker hangs every time looks busy for ever, and the export asks for ever; it asks exactly `2 * MAX_BUSY` times (a start's run and an open's run each have `MAX_BUSY`, as `agreement::asked` and the indexer's) and says the last busy answer.
    #[test]
    fn busy_answers_in_a_row_stop_at_twice_max_busy_asks() {
        let mut asks = 0u8;
        let got: Result<(), _> = asked_again(
            far(),
            &mut (),
            |_, _| {
                asks += 1;
                Err(RenderFailure::TooBusy { page: 0 })
            },
            |_| Ok(()),
        );
        assert_eq!(got, Err(RenderFailure::TooBusy { page: 0 }));
        assert_eq!(asks, 2 * MAX_BUSY);
    }

    /// Failure: one ask can be busy from three separate service runs (a start, an open, a page read, each bounded by `MAX_BUSY` on its own), so an export that gives up after `MAX_BUSY` asks says "too busy" while the next ask would have drawn the page. A read-busy, a start-busy and a read-busy in a row succeed on the next.
    #[test]
    fn three_kinds_of_busy_in_a_row_are_asked_through() {
        let answers = [
            Err(RenderFailure::TooBusy { page: 0 }),
            Err(RenderFailure::TooBusyToStart),
            Err(RenderFailure::TooBusy { page: 0 }),
            Ok(7),
        ];
        let mut asked = 0;
        let got = asked_again(
            far(),
            &mut (),
            |_, _| {
                asked += 1;
                answers[asked - 1]
            },
            |_| Ok(()),
        );
        assert_eq!(got, Ok(7));
        assert_eq!(asked, 4);
    }

    /// Failure: an export asks again past its own deadline, or asks again of a failure that is the page's own.
    #[test]
    fn nothing_is_asked_again_after_the_deadline_or_of_a_page_failure() {
        let mut asks = 0;
        let late: Result<(), _> = asked_again(
            Instant::now(),
            &mut (),
            |_, _| {
                asks += 1;
                Err(RenderFailure::TooBusyToStart)
            },
            |_| Ok(()),
        );
        assert_eq!((late, asks), (Err(RenderFailure::TooBusyToStart), 1));
        let mut asks = 0;
        let own: Result<(), _> = asked_again(
            far(),
            &mut (),
            |_, _| {
                asks += 1;
                Err(RenderFailure::NotPdf)
            },
            |_| Ok(()),
        );
        assert_eq!((own, asks), (Err(RenderFailure::NotPdf), 1));
    }

    /// Failure: every ask is given the whole of the command's time, so a busy first ask that used most of it leaves an ask again free to run as long again, and an export (or a clip's several calls) runs past the one deadline it was given. Each ask is given what is left: the second is given at least the time the first spent less than the first was.
    #[test]
    fn each_ask_is_given_only_the_time_left() {
        let (mut given, mut spent) = (Vec::new(), Duration::ZERO);
        let got: Result<(), _> = asked_again(
            Instant::now() + Duration::from_secs(60),
            &mut (),
            |_, left| {
                given.push(left);
                // Spends a few milliseconds without sleeping.
                let began = Instant::now();
                while began.elapsed() < Duration::from_millis(3) {}
                spent = began.elapsed();
                if given.len() < 2 { Err(RenderFailure::TooBusy { page: 0 }) } else { Ok(()) }
            },
            |_| Ok(()),
        );
        assert_eq!(got, Ok(()));
        assert!(given[0] - given[1] >= Duration::from_millis(3), "{given:?} after spending {spent:?}");
    }
}
