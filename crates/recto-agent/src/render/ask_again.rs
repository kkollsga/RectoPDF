//! **Asking again while the Mac is too busy,** for the exports that ask the render service once (`export svg`, an image, the list of a page's images). A busy answer ([`RenderFailure::is_busy`]) says nothing of the page and the service remembers nothing of it, so the person isn't handed "too busy, it will be tried again" with nothing trying: the export starts its output over and asks again. At most [`MAX_BUSY`] asks (the service's own bound for a page's run), and none after the command's `deadline`; the last busy answer is then the answer. The same idea as `agreement::asked` and the indexer's `asked`, which count starts and opens apart and could take this one.

use recto_render_service::{MAX_BUSY, RenderFailure};
use std::time::Instant;

/// Runs `ask` with the run's `state`; on a busy answer with asks and time left, `start_over` clears whatever the failed ask left in `state` and in the output (a failure of it ends the export as that failure), and it is asked again.
pub(crate) fn asked_again<S, T>(
    deadline: Instant,
    state: &mut S,
    mut ask: impl FnMut(&mut S) -> Result<T, RenderFailure>,
    mut start_over: impl FnMut(&mut S) -> Result<(), RenderFailure>,
) -> Result<T, RenderFailure> {
    let mut asks = 0u8;
    loop {
        asks += 1;
        match ask(state) {
            Err(busy) if busy.is_busy() && asks < MAX_BUSY && Instant::now() < deadline => start_over(state)?,
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
            |out| {
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

    /// Failure: a page whose worker hangs every time looks busy for ever, and the export asks for ever; it asks exactly `MAX_BUSY` times and says the last busy answer.
    #[test]
    fn busy_answers_in_a_row_stop_at_max_busy_asks() {
        let mut asks = 0u8;
        let got: Result<(), _> = asked_again(
            far(),
            &mut (),
            |_| {
                asks += 1;
                Err(RenderFailure::TooBusy { page: 0 })
            },
            |_| Ok(()),
        );
        assert_eq!(got, Err(RenderFailure::TooBusy { page: 0 }));
        assert_eq!(asks, MAX_BUSY);
    }

    /// Failure: an export asks again past its own deadline, or asks again of a failure that is the page's own.
    #[test]
    fn nothing_is_asked_again_after_the_deadline_or_of_a_page_failure() {
        let mut asks = 0;
        let late: Result<(), _> = asked_again(
            Instant::now(),
            &mut (),
            |_| {
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
            |_| {
                asks += 1;
                Err(RenderFailure::NotPdf)
            },
            |_| Ok(()),
        );
        assert_eq!((own, asks), (Err(RenderFailure::NotPdf), 1));
    }
}
