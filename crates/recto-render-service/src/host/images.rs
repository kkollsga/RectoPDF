//! Where a page's images are, from the host's side, for dark paper: asked of a worker once a page, as a walk over its objects that decodes nothing, and remembered, up to a cap. See `docs/engineering/paper.md`.

use super::{DocRenderer, JobFailure, KillReason, MAX_ASKING_KILLS, MAX_FAILED_PAGES, PageDrawState, RenderFailure, WorkerClass, busy, file_stamp};
use recto_proto::{self as proto, Kind, ProtoError};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Pages whose images are remembered, at 64 boxes of 8 bytes each at most. Many more than the tiles and previews kept, so a page can't have tiles on the screen from before it was forgotten.
const MAX_IMAGE_PLACES: usize = 4096;

/// How long dark paper's image checks that stayed starved for `MAX_BUSY` in a row are remembered as unknown, before one fresh walk is made. A walk costs a page deadline, so a worker that hangs on a page costs one a minute at most, and a Mac busy for a long while (a build, a wake) has its pages decided from their images soon after it's quiet.
const BUSY_REASK: Duration = Duration::from_secs(60);

/// A test's own length for [`BUSY_REASK`], returning the one it had.
#[cfg(feature = "test-seams")]
pub(crate) fn set_busy_reask(doc: &mut DocRenderer, pause: Option<Duration>) -> Option<Duration> {
    std::mem::replace(&mut doc.images.busy_reask, pause)
}

/// **What's known of pages' images,** for those asked about (out of `DocRenderer`'s fields unchanged, R4.3, to make room for the hidden highlights).
#[derive(Debug, Default)]
pub(super) struct Known {
    /// Each page's largest image in source pixels.
    traits: Vec<(u32, u64)>,
    /// Where each page's images are.
    places: HashMap<u32, Option<Vec<[u16; 4]>>>,
    /// `places` has been forgotten, past its cap, and nobody's been told.
    places_cleared: bool,
    /// Pages whose `None` in `places` is a run of starved checks, not the page's own failure: when one fresh walk may be made. Only where `places` has the page.
    busy_unknown: HashMap<u32, Instant>,
    /// A test's length for [`BUSY_REASK`].
    busy_reask: Option<Duration>,
    /// Where each page's images may be, for a click (`DocRenderer::image_boxes_for_a_click`): apart from dark paper's, so its answer is never this one.
    /// At most [`MAX_IMAGE_PLACES`] pages of 64 boxes and the one past them, 8 bytes each (520 a page): the pages on show that the tab's worker was asked about at rest ([`prefetch_click_places`], called from `recto-reader`'s `engine/run.rs`) and those clicked on.
    click_places: HashMap<u32, Option<proto::ClickPlaces>>,
    /// Pages a walk ahead of a click has been made for, whatever came of it, so it's made once and a page that failed isn't walked again at every rest. Holds no answer.
    prefetched: HashSet<u32>,
    /// A walk ahead of a click is running: a worker lost in it isn't replaced for it, and nothing starts one.
    pub(super) prefetching: bool,
}

impl Known {
    /// Whether any page's places are kept, which is when a stamp check before serving one is worth its `fstat`.
    pub(super) fn holds_any(&self) -> bool {
        !self.places.is_empty() || !self.click_places.is_empty()
    }

    /// The file is another now: nothing known of its pages holds.
    pub(super) fn forget(&mut self) {
        self.traits.clear();
        self.places.clear();
        self.busy_unknown.clear();
        self.click_places.clear();
        self.prefetched.clear();
    }
}

impl DocRenderer {
    /// The largest image on a page in source pixels, a hint for how to draw it. Remembered per page, up to a cap. If looking at the page stops a worker, that's noted as drawing would have noted
    /// it, and the answer is zero. Likewise if `generation` goes stale, without remembering that.
    pub fn largest_image_pixels(&mut self, page: u32, generation: u64) -> Result<u64, RenderFailure> {
        if let Some(&(_, pixels)) = self.images.traits.iter().find(|(known, _)| *known == page) {
            return Ok(pixels);
        }
        if self.page_draw_state(page) != PageDrawState::Sharp || self.control.is_stale(generation) || self.stopped() {
            return Ok(0);
        }
        let job_id = self.next_job_id();
        let frame = proto::encode_traits_request(job_id, page);
        let busy_before = busy::before(self);
        // Loads this page, which is what the next job for it needs anyway, so it leaves nothing that could explain that job failing.
        let reply = self.run_job(job_id, page, false, WorkerClass::Normal, Some(generation), &[], &frame, |reply| {
            match reply.kind {
                Kind::Traits => {
                    let traits = proto::PageTraits::decode(&reply.payload)?;
                    if traits.job_id == job_id {
                        Ok(traits.largest_image_pixels)
                    } else {
                        Err(ProtoError("traits answer a different job"))
                    }
                }
                _ => Err(ProtoError("the wrong kind of reply to a traits request")),
            }
        });
        let pixels = match reply {
            Ok(pixels) => {
                busy::landed(self, page);
                pixels
            }
            Err(JobFailure::StaleKilled) => return Ok(0),
            // At what scale isn't known, so it holds at every one, until a lower zoom's own draw says otherwise. Which is the same as the page's load being too heavy.
            Err(JobFailure::Stopped(KillReason::MemoryLimit)) => {
                self.set_draw(page, 0.0, PageDrawState::NeedsHeavy);
                0
            }
            Err(JobFailure::Stopped(reason)) => return Err(busy::draw_stopped(self, page, 0.0, reason, busy_before)),
            Err(failure) => return Err(failure.into()),
        };
        if self.images.traits.len() >= MAX_FAILED_PAGES {
            self.images.traits.remove(0);
        }
        self.images.traits.push((page, pixels));
        Ok(pixels)
    }

    /// Where a page's images are, as parts of the page in 65535ths, or `None` if it isn't known: this job went stale, the page isn't one that can be asked about, or the asking failed or was refused, as a page with more images than an answer holds is. Those aren't "there are no images", and a caller mustn't treat them so. Asked once per page, up to a cap, only for dark paper. Whatever stops the answer gives none, since it only decides how pixels are coloured, and drawing will find the same trouble and deal with it. A check the Mac was too busy for (starved, or not started) is also `None` and is NOT remembered, so it's asked again, until `MAX_BUSY` of them in a row, after which it's an answer.
    pub fn image_boxes(&mut self, page: u32, generation: u64) -> Option<Vec<[u16; 4]>> {
        forget_places_of_a_replaced_file(self);
        if let Some(known) = self.images.places.get(&page) {
            if !self.images.busy_unknown.get(&page).is_some_and(|at| Instant::now() >= *at) {
                return known.clone();
            }
            // Its time: one fresh walk, with a new run of busy ones should it be starved again.
            self.images.places.remove(&page);
            self.images.busy_unknown.remove(&page);
            busy::landed(self, page);
        }
        // Stale says nothing about the page, so nothing's remembered.
        let boxes = places_asked(self, page, generation, Ask::Dark, |payload| {
            proto::PageImages::decode(payload).map(|images| (images.job_id, images.boxes))
        })?;
        // One decision a page: its tiles are drawn at different times, and none of them may disagree. What's cached past this cap is long off the screen.
        if self.images.places.len() >= MAX_IMAGE_PLACES {
            self.images.places.clear();
            self.images.busy_unknown.clear();
            self.images.places_cleared = true;
        }
        self.images.places.insert(page, boxes.clone());
        boxes
    }

    /// **Where a page's images may be, for a click** (the reader's image picked with a click): a superset, never too many, the walk's answer to a request
    /// with the click's flag (`places.rs`), typed as it's decoded: the pictures' boxes, each by its own box through the forms it's in, and apart from
    /// them the box where a picture may be anywhere, which is never read as one. Never dark paper's boxes, which give a picture in a form the outermost
    /// form's box. `None` if it isn't known: stale (nothing remembered), a check the Mac was too busy for (starved or not started: not remembered either, until `MAX_BUSY` in a row), or the page couldn't be asked or the asking failed, which a click takes as no
    /// picture there. Remembered apart from dark paper's, under the same cap.
    pub fn image_boxes_for_a_click(&mut self, page: u32, generation: u64) -> Option<proto::ClickPlaces> {
        forget_places_of_a_replaced_file(self);
        if let Some(known) = self.images.click_places.get(&page) {
            return known.clone();
        }
        let places = places_asked(self, page, generation, Ask::Click, |payload| {
            proto::PageImagesForAClick::decode(payload).map(|images| (images.job_id, images.places))
        })?;
        if self.images.click_places.len() >= MAX_IMAGE_PLACES {
            self.images.click_places.clear();
        }
        self.images.click_places.insert(page, places.clone());
        places
    }

    /// Whether the places of a page's images have been forgotten since this was last asked, with a lot of pages looked up. It says nothing about the file: it's for whoever decided something from those places, which might not be decided the same way twice.
    pub fn take_image_places_cleared(&mut self) -> bool {
        std::mem::take(&mut self.images.places_cleared)
    }
}

/// **What's kept of the file's pictures isn't served for a file another app has rewritten.** An answer kept is given with no job, so the stamp check every job makes
/// ([`DocRenderer::reopen_if_replaced`], which forgets them, as it does when a worker finds the file changed) wouldn't have run: a click after a rewrite got the old file's "no
/// picture". One `fstat` (about a microsecond), and only when something's kept; a miss goes to a job, which checks as every job does.
fn forget_places_of_a_replaced_file(doc: &mut DocRenderer) {
    if doc.images.holds_any() {
        doc.reopen_if_replaced();
    }
}

/// **Reads a page's click places ahead of a click,** so one after the tab's worker has been let go at rest is answered from what's kept and starts no worker. It runs only where
/// it can't cost anything the person would see, and keeps only an answer:
/// - only with the worker that has the page alive (it never starts one, and one lost in the walk isn't replaced for it), for a page it looked at (`largest_image_pixels`
///   came back) that draws as any (not one needing the larger limit, nor one that failed), of a file as it was opened, in a renderer not stopped;
/// - a failure of any kind, a kill, a refusal or a cap, keeps nothing: the click asks as it would have without this, and a click's own failed walk is kept as it ever was;
/// - its kills are counted as the document's own kills are for a check (left out of what stops the document) but not towards the 16 that dark paper's walk and a click's share
///   ([`MAX_ASKING_KILLS`]), as scrolling on white paper mustn't end dark paper's asking.
///
/// Says whether a walk was made, so the thread that draws can go on to the next page, or rest.
pub fn prefetch_click_places(doc: &mut DocRenderer, page: u32, generation: u64) -> bool {
    forget_places_of_a_replaced_file(doc);
    doc.reap_if_gone();
    let looked = doc.images.traits.iter().any(|(known, _)| *known == page);
    let unchanged = file_stamp(&doc.path).ok() == doc.opened_stamp;
    let fit = doc.worker.is_some()
        && looked
        && unchanged
        && doc.page_draw_state(page) == PageDrawState::Sharp
        && !doc.stopped()
        && !doc.control.is_stale(generation)
        && !doc.images.click_places.contains_key(&page)
        && !doc.images.prefetched.contains(&page);
    if !fit {
        return false;
    }
    if doc.images.prefetched.len() >= MAX_IMAGE_PLACES {
        doc.images.prefetched.clear();
    }
    doc.images.prefetched.insert(page);
    doc.images.prefetching = true;
    let asked = places_asked(doc, page, generation, Ask::Prefetch, |payload| {
        proto::PageImagesForAClick::decode(payload).map(|images| (images.job_id, images.places))
    });
    doc.images.prefetching = false;
    // Only an answer is kept: stale and failed say nothing about the page.
    if let Some(Some(places)) = asked {
        if doc.images.click_places.len() >= MAX_IMAGE_PLACES {
            doc.images.click_places.clear();
        }
        doc.images.click_places.insert(page, Some(places));
    }
    true
}

/// Whose walk it is: dark paper's, a click's, or one ahead of a click's ([`prefetch_click_places`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Dark,
    Click,
    Prefetch,
}

/// Asks `doc`'s worker where a page's images are, dark paper's way or a click's, its answer read by `decode`, the one for that request's shape, so a
/// worker answering with the other's is a violation (a function of the renderer's, not one of its methods, which are many): `None` if `generation` went
/// stale, or a check the Mac was too busy for (starved or not started) was stopped, which say nothing about the page and are not remembered, until `MAX_BUSY` in a row.
fn places_asked<T>(doc: &mut DocRenderer, page: u32, generation: u64, ask: Ask, decode: impl Fn(&[u8]) -> Result<(u32, T), ProtoError>) -> Option<Option<T>> {
    if doc.control.is_stale(generation) {
        return None;
    }
    // A page whose drawing failed for time or a crash goes unasked: that's remembered at every zoom. Asking walks its objects and decodes nothing, so one needing the larger limit, drawn lower, or failed for memory even at its lower resolution (remembered from that scale down, not at every zoom), is asked as any page, and its look doesn't hang on which draw went first.
    let state = doc.page_draw_state(page);
    let unaskable = matches!(doc.page_draw_state_at(page, 0.0), PageDrawState::Failed(_));
    if unaskable || doc.stopped() || (ask != Ask::Prefetch && doc.left_out.walk >= MAX_ASKING_KILLS) {
        return Some(None);
    }
    // The worker that's there, which has just drawn it. With none, one that can hold the page.
    let class = if doc.worker.is_none() && state == PageDrawState::NeedsHeavy {
        WorkerClass::Heavy
    } else {
        WorkerClass::Normal
    };
    let job_id = doc.next_job_id();
    let frame = if ask != Ask::Dark {
        proto::encode_images_request_for_a_click(job_id, page)
    } else {
        proto::encode_images_request(job_id, page)
    };
    let (kills, busy) = (doc.kills(), doc.left_out.busy);
    let busy_before = busy::before(doc);
    let reply = doc.run_job(job_id, page, false, class, Some(generation), &[], &frame, |reply| match reply.kind {
        Kind::Images => {
            let (answered, places) = decode(&reply.payload)?;
            if answered == job_id {
                Ok(places)
            } else {
                Err(ProtoError("image boxes answer a different job"))
            }
        }
        _ => Err(ProtoError("the wrong kind of reply to an images request")),
    });
    // A worker killed walking the page is left out of what stops the document, as the hidden-text check's are, and counts towards the walk's own cap.
    let left_out = doc.left_out.since(doc.kills() - kills, busy);
    if ask == Ask::Prefetch {
        doc.left_out.prefetch = doc.left_out.prefetch.saturating_add(left_out);
    } else {
        doc.left_out.walk = doc.left_out.walk.saturating_add(left_out);
    }
    // Stale: not about the page, so nothing's remembered.
    if doc.control.is_stale(generation) {
        return None;
    }
    // A check the Mac was too busy for says nothing about the page, so nothing's remembered and whoever asked asks again; past `MAX_BUSY` in a row it's an answer after all.
    if busy::walk_stopped(doc, page, &reply, busy_before) {
        return None;
    }
    if reply.is_ok() {
        busy::landed(doc, page);
    } else if ask == Ask::Dark && busy::walk_was_starved(&reply, busy_before, doc) {
        // The run reached its bound: unknown, but only for a while.
        let pause = doc.images.busy_reask.unwrap_or(BUSY_REASK);
        doc.images.busy_unknown.insert(page, Instant::now() + pause);
    }
    Some(reply.ok())
}
