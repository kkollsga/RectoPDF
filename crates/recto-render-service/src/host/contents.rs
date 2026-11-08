//! A page's contents, from the host's side: asks for a frame at a time, checks what comes, and hands it on. Goes down the same limits as an export, so a page comes out whole or is refused, and nothing is written.

use super::{DocRenderer, ExportRefusal, JobFailure, KillReason, PageDrawState, RenderFailure, SlotWaitEnd, WorkerClass};
use recto_proto::jpeg::JpegFacts;
use recto_proto::contents::{ContentsJob, Record, Records, StreamCheck, TextMode, decode_contents, validate_contents};
use recto_proto::{Kind, ProtoError};
use std::time::{Duration, Instant};

/// What's wanted of a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentsRequest {
    pub page: u32,
    pub generation: u64,
    /// The scale pictures are drawn at, where a picture has to stand in.
    pub scale: f32,
    /// Part of the page, in points from its top left: `x, y, width, height`. `None` is the whole page.
    pub rect: Option<[f32; 4]>,
    pub text: TextMode,
    /// What the file's structure says the page uses: [`recto_proto::contents::hints`].
    pub hints: u8,
    /// Everything, where each image is, or one image: [`recto_proto::contents::IMAGES_ALL`] and what follows it.
    pub images: u32,
    /// Nothing more than this is spent on the page, time asleep waiting for the larger limit aside.
    pub max_time: Duration,
}

/// What a page's contents are, as they come.
#[derive(Clone, Copy, Debug)]
pub enum ContentsEvent<'a> {
    /// Forget everything so far. A worker was stopped, and it's starting again from the top in a larger one.
    Restart,
    /// The next record. It's been checked against everything before it.
    Record(Record<'a>),
}

/// How a page's contents ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentsDone {
    pub frames: u32,
    pub restarts: u32,
    /// What the JPEGs in it carry besides their pictures.
    pub jpeg: JpegFacts,
}

/// How many times a page may lose its place and start again, inside one class of worker.
const MAX_RESTARTS: u32 = 2;

/// How one try ended.
enum TryEnd {
    Finished,
    Stopped(KillReason),
    LostPlace,
}

impl DocRenderer {
    /// Reads a page's contents, a frame at a time, as an export: sharp or refused, and never a lower resolution, since a picture standing in is drawn at the scale asked for. `sink` hands the records on, and a failure from it ends things there. A failure here means nothing should be published. Can block for a long time, so never on a UI thread.
    pub fn read_contents(&mut self, request: &ContentsRequest, mut sink: impl FnMut(ContentsEvent<'_>) -> Result<(), RenderFailure>) -> Result<ContentsDone, RenderFailure> {
        let page = request.page;
        let check = job_of(request, 0, 0);
        check.check().map_err(|_| RenderFailure::BadRequest)?;
        let started = Instant::now();
        let waited_before = self.time_waiting_for_slot();
        let mut done = ContentsDone::default();
        let mut began = false;
        self.reap_if_gone();
        self.retire_if_other_page(page);
        let scale = request.scale;
        loop {
            if self.control.is_stale(request.generation) {
                return Err(RenderFailure::Cancelled);
            }
            let state = self.page_draw_state_at(page, scale);
            if !matches!(state, PageDrawState::Failed(_)) && self.stopped() {
                return Err(RenderFailure::DocumentStopped);
            }
            let class = match state {
                PageDrawState::Failed(reason) => return Err(RenderFailure::PageFailed { page, reason }),
                PageDrawState::LowResOnly => return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::TooHeavy }),
                PageDrawState::Sharp => WorkerClass::Normal,
                PageDrawState::NeedsHeavy => {
                    let retries = self.stats.heavy_retries - self.heavy_retries_counted_from;
                    let already_heavy = self.worker.as_ref().is_some_and(|worker| worker.heavy);
                    if self.policy.heavy_limit_bytes <= self.policy.memory_limit_bytes || (!already_heavy && retries >= self.policy.max_heavy_retries) {
                        self.set_draw(page, scale, PageDrawState::LowResOnly);
                        continue;
                    }
                    if !already_heavy && !self.memory_to_spare() {
                        return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::MemoryTight });
                    }
                    if !already_heavy && self.waited_ticket.is_none() {
                        match self.wait_for_slot(request.generation) {
                            Ok(ticket) => self.waited_ticket = Some(ticket),
                            Err(SlotWaitEnd::Cancelled) => return Err(RenderFailure::Cancelled),
                            Err(SlotWaitEnd::TimedOut) => return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::WaitTimedOut }),
                        }
                        if !self.memory_to_spare() {
                            self.waited_ticket = None;
                            return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::MemoryTight });
                        }
                    }
                    WorkerClass::Heavy
                }
            };
            if began {
                done.restarts += 1;
                sink(ContentsEvent::Restart)?;
            }
            began = true;
            let tried = self.try_contents(request, class, started, waited_before, &mut done, &mut sink);
            self.waited_ticket = None;
            match tried {
                Ok(TryEnd::Finished) => return Ok(done),
                Ok(TryEnd::LostPlace) => {
                    // A worker gave up its place for no reason of the page's, so it starts again. A worker that keeps doing it is broken, or hostile.
                    if done.restarts >= MAX_RESTARTS {
                        return Err(self.give_up_drawing(page, 0.0, KillReason::Rejected("a worker that keeps losing its place")));
                    }
                }
                Ok(TryEnd::Stopped(KillReason::MemoryLimit)) => {
                    let next = if class == WorkerClass::Heavy { PageDrawState::LowResOnly } else { PageDrawState::NeedsHeavy };
                    self.set_draw(page, scale, next);
                }
                Ok(TryEnd::Stopped(reason)) => return Err(self.give_up_drawing(page, 0.0, reason)),
                Err(JobFailure::Failed(failure)) => return Err(failure),
                Err(JobFailure::StaleKilled) => return Err(RenderFailure::Cancelled),
                Err(JobFailure::Stopped(KillReason::MemoryLimit)) => {
                    let next = if class == WorkerClass::Heavy { PageDrawState::LowResOnly } else { PageDrawState::NeedsHeavy };
                    self.set_draw(page, scale, next);
                }
                Err(JobFailure::Stopped(reason)) => return Err(self.give_up_drawing(page, 0.0, reason)),
                // Its worker left between frames and someone else has the slot now. Back to waiting, within the same limits.
                Err(JobFailure::HeavyBusy) => {}
            }
        }
    }

    fn try_contents(
        &mut self,
        request: &ContentsRequest,
        class: WorkerClass,
        started: Instant,
        waited_before: Duration,
        done: &mut ContentsDone,
        sink: &mut impl FnMut(ContentsEvent<'_>) -> Result<(), RenderFailure>,
    ) -> Result<TryEnd, JobFailure> {
        let page = request.page;
        let mut check = StreamCheck::for_images(request.images);
        let mut seq = 0u32;
        loop {
            if started.elapsed().saturating_sub(self.time_waiting_for_slot().saturating_sub(waited_before)) >= request.max_time {
                return Err(RenderFailure::TookTooLong.into());
            }
            if self.control.is_stale(request.generation) {
                return Err(RenderFailure::Cancelled.into());
            }
            let job = job_of(request, self.next_job_id(), seq);
            let frame = job.encode();
            // A stream's own frames don't count as jobs done: they're all this one page, so a kill on one of them isn't what earlier pages left behind, and it isn't worth another try at the same size. A worker with real history gets that retry, from the top.
            let reply = self.run_job(job.job_id, page, false, class, Some(request.generation), &[], &frame, |reply| match reply.kind {
                Kind::Contents => {
                    let (header, _) = decode_contents(&reply.payload)?;
                    validate_contents(&job, &header)?;
                    // Taken, not copied: a frame can be 16 MiB.
                    Ok(std::mem::take(&mut reply.payload))
                }
                _ => Err(ProtoError("the wrong kind of reply to a request for contents")),
            });
            let payload = match reply {
                Ok(payload) => payload,
                // A fresh worker has no place to go on from, as when one was retried under it. That's a job the caller starts again.
                Err(JobFailure::Failed(RenderFailure::BadRequest)) if seq > 0 => return Ok(TryEnd::LostPlace),
                Err(JobFailure::Stopped(reason)) => return Ok(TryEnd::Stopped(reason)),
                Err(failure) => return Err(failure),
            };
            let Ok((header, records)) = decode_contents(&payload) else { return Err(RenderFailure::Unavailable.into()) };
            let mut reader = Records::new(records);
            loop {
                let record = match reader.next_record() {
                    Ok(Some(record)) => record,
                    Ok(None) => break,
                    Err(error) => return self.reject_contents(error),
                };
                if let Err(error) = check.accept(&record) {
                    return self.reject_contents(error);
                }
                sink(ContentsEvent::Record(record))?;
            }
            done.frames += 1;
            if header.done {
                if let Err(error) = check.finish() {
                    return self.reject_contents(error);
                }
                done.jpeg = check.jpeg_facts();
                return Ok(TryEnd::Finished);
            }
            seq = seq.wrapping_add(1);
        }
    }

    /// What a worker sent breaks the rules, so it's stopped, as any violation is.
    fn reject_contents(&mut self, error: ProtoError) -> Result<TryEnd, JobFailure> {
        self.stats.rejections += 1;
        self.bury_worker();
        Err(JobFailure::Stopped(KillReason::Rejected(error.0)))
    }
}

fn job_of(request: &ContentsRequest, job_id: u32, seq: u32) -> ContentsJob {
    ContentsJob { generation: request.generation, job_id, page: request.page, seq, scale: request.scale, rect: request.rect.unwrap_or([0.0; 4]), text: request.text, hints: request.hints, images: request.images }
}
