//! Where a page's images are, from the host's side, for dark paper: asked of a worker once a page, as a walk over its objects that decodes nothing, and remembered, up to a cap. See `docs/engineering/paper.md`.

use super::{DocRenderer, JobFailure, MAX_ASKING_KILLS, PageDrawState, RenderFailure, WorkerClass};
use recto_proto::{self as proto, Kind, ProtoError};

/// Pages whose images are remembered, at 64 boxes of 8 bytes each at most. Many more than the tiles and previews kept, so a page can't have tiles on the screen from before it was forgotten.
const MAX_IMAGE_PLACES: usize = 4096;

impl DocRenderer {
    /// Where a page's images are, as parts of the page in 65535ths, or `None` if it isn't known: this job went stale, the page isn't one that can be asked about, or the asking failed or was refused, as a page with more images than an answer holds is. Those aren't "there are no images", and a caller mustn't treat them so. Asked once per page, up to a cap, only for dark paper. Whatever stops the answer gives none, since it only decides how pixels are coloured, and drawing will find the same trouble and deal with it. `Err` is only [`RenderFailure::WouldStart`]: not asked, as the job may use only the worker that's there ([`super::Workers::OnlyTheOneThere`]) and that one couldn't. That's no answer about the page either, and a caller mustn't decide anything from it and keep it, as it would from `None`.
    pub fn image_boxes(&mut self, page: u32, generation: u64) -> Result<Option<Vec<[u16; 4]>>, RenderFailure> {
        if let Some(known) = self.image_places.get(&page) {
            return Ok(known.clone());
        }
        // Stale says nothing about the page, so nothing's remembered.
        if self.control.is_stale(generation) {
            return Ok(None);
        }
        // A page whose drawing failed for time or a crash goes unasked: that's remembered at every zoom. Asking walks its objects and decodes nothing, so one needing the larger limit, drawn lower, or failed for memory even at its lower resolution (remembered from that scale down, not at every zoom), is asked as any page, and its look doesn't hang on which draw went first.
        let state = self.page_draw_state(page);
        let unaskable = matches!(self.page_draw_state_at(page, 0.0), PageDrawState::Failed(_));
        let boxes = if unaskable || self.stopped() || self.left_out.walk >= MAX_ASKING_KILLS {
            None
        } else {
            // The worker that's there, which has just drawn it. With none, one that can hold the page.
            let class = if self.worker.is_none() && state == PageDrawState::NeedsHeavy {
                WorkerClass::Heavy
            } else {
                WorkerClass::Normal
            };
            let job_id = self.next_job_id();
            let frame = proto::encode_images_request(job_id, page);
            let (kills, busy) = (self.kills(), self.left_out.busy);
            let reply = self.run_job(job_id, page, false, class, Some(generation), &[], &frame, |reply| match reply.kind {
                Kind::Images => {
                    let images = proto::PageImages::decode(&reply.payload)?;
                    if images.job_id == job_id {
                        Ok(images.boxes)
                    } else {
                        Err(ProtoError("image boxes answer a different job"))
                    }
                }
                _ => Err(ProtoError("the wrong kind of reply to an images request")),
            });
            // A worker killed walking the page is left out of what stops the document, as the hidden-text check's are, and counts towards the walk's own cap.
            self.left_out.walk = self.left_out.walk.saturating_add(self.left_out.since(self.kills() - kills, busy));
            // Stale, or not asked for want of a worker: neither is about the page, so nothing's remembered.
            match reply {
                _ if self.control.is_stale(generation) => return Ok(None),
                Err(JobFailure::Failed(failure @ RenderFailure::WouldStart { .. })) => return Err(failure),
                reply => reply.ok(),
            }
        };
        // One decision a page: its tiles are drawn at different times, and none of them may disagree. What's cached past this cap is long off the screen.
        if self.image_places.len() >= MAX_IMAGE_PLACES {
            self.image_places.clear();
            self.image_places_cleared = true;
        }
        self.image_places.insert(page, boxes.clone());
        Ok(boxes)
    }

    /// Whether the places of a page's images have been forgotten since this was last asked, with a lot of pages looked up. It says nothing about the file: it's for whoever decided something from those places, which might not be decided the same way twice.
    pub fn take_image_places_cleared(&mut self) -> bool {
        std::mem::take(&mut self.image_places_cleared)
    }
}
