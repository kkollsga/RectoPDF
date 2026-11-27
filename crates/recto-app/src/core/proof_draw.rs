//! **The new version proves it can draw** before a launch an update opened is called good: our own one-colour page, through the real renderer.
//!
//! A Mac too busy to start the renderer says nothing of this version, so a busy answer is "not proven yet" and the proof asks again, inside [`PATIENCE`]. Only
//! a failure that isn't busy (the walls down, a crash, a refusal, a wrong colour) or the patience running out is "can't draw", which sends an update back.

use recto_platform::isolate::SharedMemory;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig, TileKind, TileRect, TileRequest, TileResult};
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a busy Mac is asked again: a quarter of the installer's wait for a new version (`recto_update::plan::START_LIMIT_MS`), which is the time the
/// launch's own tries (a minute, `lifecycle::keeping`) leave for the drawing of a page. No timer of its own beyond what the installer already gives.
pub const PATIENCE: Duration = Duration::from_millis(recto_update::plan::START_LIMIT_MS / 4);

/// What one try at drawing the page came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Try {
    /// The page came back, and it was (`true`) or wasn't the colour it should be.
    Drew(bool),
    /// The Mac was too busy to start the renderer, or to open the page, for this try. Says nothing of this version.
    Busy,
    /// Anything else: this version can't.
    Broke,
}

/// Whether a failure is the Mac's. `busy_seen` is the renderer's own count of busy starts and opens in this try: past its [`MAX_BUSY`](recto_render_service::MAX_BUSY)
/// in a row the service gives `Unavailable` and forgets that it was the Mac, so the count is what says it was.
#[must_use]
pub fn failure_try(failure: RenderFailure, busy_seen: bool) -> Try {
    if failure.is_busy() || (busy_seen && matches!(failure, RenderFailure::Unavailable)) {
        Try::Busy
    } else {
        Try::Broke
    }
}

/// Asks again while a try is busy, until `patience` has passed since `began`: `true` only for a page that came back right. `pause` is told to wait between busy tries.
pub fn prove(began: Instant, patience: Duration, mut attempt: impl FnMut() -> Try, mut pause: impl FnMut()) -> bool {
    loop {
        match attempt() {
            Try::Drew(right) => return right,
            Try::Broke => return false,
            Try::Busy if began.elapsed() >= patience => return false,
            Try::Busy => pause(),
        }
    }
}

/// Whether this version can draw: the page below, through a real worker with the real library, and what comes back has to be that colour. What a new version has to show before it counts as having come up, as nothing else says a renderer that can't load is broken until someone's looking at a blank page.
pub fn draws_a_page(folder: &Path) -> bool {
    let file = folder.join("draws.pdf");
    if std::fs::create_dir_all(folder).is_err() || std::fs::write(&file, probe_pdf()).is_err() {
        return false;
    }
    // A renderer of its own for each try: what a busy run left remembered in the last one isn't carried into the next.
    let pause = || std::thread::sleep(Duration::from_millis(250)); // ratchet: poll
    let drew = prove(Instant::now(), PATIENCE, || one_try(&file), pause);
    let _ = std::fs::remove_file(&file);
    drew
}

fn one_try(file: &Path) -> Try {
    let Ok(config) = ServiceConfig::discover() else { return Try::Broke };
    let mut document = match DocRenderer::new(file, None, config, Policy::default()) {
        Ok(document) => document,
        Err(failure) => return failure_try(failure, false),
    };
    let Ok(surface) = SharedMemory::create(64 * 64 * 4) else { return Try::Broke };
    let request = TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
        scale: 1.0,
        stride: 64 * 4,
        generation: 0,
    };
    match document.render_tile(&request, &surface) {
        Ok(TileResult::Done(tile)) => Try::Drew(matches!(tile.kind, TileKind::Blank { rgb } if (rgb >> 16) & 0xFF > 0xC0 && rgb & 0xFFFF < 0x4040)),
        Ok(TileResult::Cancelled { .. }) => Try::Broke,
        Err(failure) => failure_try(failure, document.stats().too_busy > 0),
    }
}

/// A page of one colour, as small as a PDF gets. Ours, so it says something about this version and nothing about whatever the user had open.
pub(super) fn probe_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Contents 4 0 R >>".to_owned(),
        {
            let content = "1 0 0 rg 0 0 64 64 re f";
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
        },
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut places = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        places.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let table = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for place in places {
        pdf.extend_from_slice(format!("{place:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a good update was sent back because the Mac was too busy to start the renderer for a few tries ("can't draw", 2026-11-27, load 13). Busy tries ask again and the first page that comes back proves it.
    #[test]
    fn a_busy_mac_is_not_proven_yet_and_the_first_page_proves_it() {
        let mut tries = vec![Try::Busy, Try::Busy, Try::Busy, Try::Busy, Try::Drew(true)].into_iter();
        let mut pauses = 0;
        assert!(prove(Instant::now(), Duration::from_secs(60), || tries.next().unwrap(), || pauses += 1));
        assert_eq!(pauses, 4);
    }

    /// Failure: asking again for ever. A Mac busy past the patience is "can't draw", and a try isn't asked once the patience is over.
    #[test]
    fn busy_past_the_patience_cant_draw() {
        let mut asked = 0;
        assert!(!prove(
            Instant::now(),
            Duration::ZERO,
            || {
                asked += 1;
                Try::Busy
            },
            || {}
        ));
        assert_eq!(asked, 1);
    }

    /// Failure: a build that really can't draw waited out the patience too. A failure that isn't busy, or a wrong colour, says so at once.
    #[test]
    fn a_failure_that_is_not_busy_cant_draw_at_once() {
        for first in [Try::Broke, Try::Drew(false)] {
            let mut asked = 0;
            let mut first = Some(first);
            let drew = prove(
                Instant::now(),
                Duration::from_secs(60),
                || {
                    asked += 1;
                    first.take().unwrap_or(Try::Busy)
                },
                || panic!("no second ask"),
            );
            assert!(!drew);
            assert_eq!(asked, 1);
        }
    }

    /// Failure: the service's own `Unavailable` after a run of busy starts lost the reason, and read as a broken build. With the busy count it is busy; without, it is not.
    #[test]
    fn unavailable_after_busy_starts_is_still_busy() {
        assert_eq!(failure_try(RenderFailure::TooBusyToStart, false), Try::Busy);
        assert_eq!(failure_try(RenderFailure::OutOfResources, false), Try::Busy);
        assert_eq!(failure_try(RenderFailure::Unavailable, true), Try::Busy);
        assert_eq!(failure_try(RenderFailure::Unavailable, false), Try::Broke);
        assert_eq!(failure_try(RenderFailure::NotPdf, true), Try::Broke);
    }
}
