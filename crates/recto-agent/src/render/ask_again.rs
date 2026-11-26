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

    /// Failure (the person saw "too busy to draw page 1. It will be tried again." and nothing tried again): an ask the Mac was too busy for (a worker with no processor at all, made by `kill -STOP`) ended an export as `page_mac_busy`, though a second ask on a fresh worker draws the page. Only the first ask is under the short page deadline (the seam wraps that ask alone, so a loaded runner can't starve the asks after it: the whole-export version of this test failed 6 of 20 under like-ci for that reason). The second ask is answered, the Mac is counted busy once, and the output was started over between them.
    #[test]
    fn a_real_worker_the_mac_was_too_busy_for_is_asked_again() {
        use recto_render_service::{DocRenderer, Policy, ServiceConfig};
        let file = recto_testpdf::TempPdf::new(
            "busy-ask.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"]))]),
        );
        let mut doc = DocRenderer::new(&file.0, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
        doc.page_size(0).unwrap();
        let pid = doc.worker_pid().expect("a worker with the file open");
        let signal = |signal: &str| {
            std::process::Command::new("/bin/kill")
                .arg(signal)
                .arg(pid.to_string())
                .status()
                .is_ok_and(|status| status.success())
        };
        assert!(signal("-STOP"));
        let (mut asks, mut overs) = (0u32, 0u32);
        let got = asked_again(
            far(),
            &mut doc,
            |doc| {
                asks += 1;
                if asks == 1 {
                    recto_render_service::under_page_deadline_for_test(doc, |doc| doc, Duration::from_millis(250), |doc| doc.page_size(0))
                } else {
                    doc.page_size(0)
                }
            },
            |_| {
                overs += 1;
                Ok(())
            },
        );
        signal("-CONT");
        assert!(got.is_ok(), "{got:?}, {:?}", doc.stats());
        assert_eq!((asks, overs, doc.stats().too_busy), (2, 1, 1), "{:?}", doc.stats());
    }
}
