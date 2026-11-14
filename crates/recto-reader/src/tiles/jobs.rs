//! **What's asked of the renderer:** the jobs, what's running and how to stop it, which files keep a renderer and which must open again, and what's come of it all.

use super::Reader;
use crate::layout::PageBox;
use crate::paper::PaperKind;
use recto_platform::surface::PixelSurface;
use recto_proto::{MAX_REGION_BYTES, TILE_EDGE, TileRect};
use recto_render_service::{CancelHandle, LowResWhy, TileKind};
use std::path::PathBuf;
use std::sync::Arc;

/// Files remembered as needing a password the renderer has none for, until the app asks: no more than tabs that could be showing.
pub(super) const MAX_KEYLESS: usize = 8;

/// One thing for the engine to do. The surface, if any, is the job's until it comes back.
#[derive(Debug)]
pub struct Job {
    pub tab: u32,
    pub position: u32,
    pub file: PathBuf,
    pub source_page: u32,
    pub generation: u64,
    pub kind: JobKind,
}

#[derive(Debug)]
pub enum JobKind {
    /// Ask a page's size.
    Size,
    /// Ask whether a page is heavy.
    Traits,
    Draw {
        scale: f32,
        rect: TileRect,
        surface: PixelSurface,
        what: DrawKind,
        paper: PaperKind,
    },
    /// Read a page's text for selecting. `check` also draws it, for the hidden-text check.
    Text { check: bool },
    /// Read a page's links, from the file's structure, with no worker.
    Links,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    Preview,
    Tile,
    /// One pass, cut on a grid of this many pixels.
    Region,
}

/// A job, before its surface is made. The reader's lock is held for as long as it takes the UI thread's `set_viewport` to get a word in, so what can take a while is done after it's let go.
#[derive(Debug)]
pub struct JobPlan {
    pub tab: u32,
    pub position: u32,
    pub file: PathBuf,
    pub source_page: u32,
    pub generation: u64,
    pub(super) kind: PlanKind,
}

#[derive(Debug)]
pub(super) enum PlanKind {
    Size,
    Traits,
    Draw {
        scale: f32,
        rect: TileRect,
        what: DrawKind,
        paper: PaperKind,
    },
    Text {
        check: bool,
    },
    Links,
}

impl JobPlan {
    /// Makes its surface. A new one for each job: see [`Self::take_surface`]. `None` if it can't be made, which leaves things as if there'd been nothing to do.
    pub fn ready(self) -> Option<Job> {
        let Self {
            tab,
            position,
            file,
            source_page,
            generation,
            kind,
        } = self;
        let kind = match kind {
            PlanKind::Size => JobKind::Size,
            PlanKind::Traits => JobKind::Traits,
            PlanKind::Text { check } => JobKind::Text { check },
            PlanKind::Links => JobKind::Links,
            PlanKind::Draw { scale, mut rect, what, paper } => {
                let surface = match what {
                    DrawKind::Preview => Self::take_surface(rect.width, rect.height)?,
                    DrawKind::Tile => Self::take_surface(TILE_EDGE, TILE_EDGE)?,
                    DrawKind::Region => loop {
                        let surface = Self::take_surface(rect.width, rect.height)?;
                        // A surface's rows can be padded, which can take it over the cap. A row of tiles less is always under.
                        if u64::from(surface.stride()) * u64::from(rect.height) <= MAX_REGION_BYTES || rect.height <= TILE_EDGE {
                            break surface;
                        }
                        rect.height -= TILE_EDGE;
                    },
                };
                JobKind::Draw {
                    scale,
                    rect,
                    surface,
                    what,
                    paper,
                }
            }
        };
        Some(Job {
            tab,
            position,
            file,
            source_page,
            generation,
            kind,
        })
    }

    /// A new surface for each job, never one that's been used. A worker can read and write a surface it was sent for as long as it lives, whatever it's told, so a surface used twice is a way from one file's worker to another file's pixels. It costs 51 µs more a tile than reusing one, nearly all of it the first touch of its pages: 67 against 16, p50.
    fn take_surface(width: u32, height: u32) -> Option<PixelSurface> {
        PixelSurface::create(width, height).ok()
    }
}

/// How a job came out.
#[derive(Debug)]
pub enum Outcome {
    Size(PageBox),
    Heavy(bool),
    Drawn {
        kind: TileKind,
        low_res: Option<(TileRect, LowResWhy)>,
    },
    Cancelled,
    /// Not done: only the worker there could do it, and it would have taken another, a heavy one if `heavy`. Only for a tab off screen's place ([`super::sleep_draw`]).
    Unstarted {
        heavy: bool,
    },
    /// The page can't be drawn now or ever, in these words.
    PageFailed {
        message: String,
        may_pass: bool,
    },
    /// The file can't be drawn, in these words. `needs_key`: because it needs a password the renderer wasn't given.
    DocumentFailed {
        message: String,
        needs_key: bool,
    },
    /// A page's text, for selecting. `checked` says a hidden-text check was asked for and has been tried, however it came out, and not stopped because nobody wanted it.
    Text {
        text: Arc<recto_index::select::PageText>,
        checked: bool,
    },
    /// A page's text can't be read. That's the page's text, and nothing about drawing it.
    TextFailed,
    /// A page's links, or none if they couldn't be read.
    Links(Option<Arc<recto_core::nav::PageLinks>>),
}

#[derive(Debug)]
pub(super) struct Running {
    pub(super) tab: u32,
    pub(super) position: u32,
    pub(super) rect: Option<TileRect>,
    pub(super) scale: f32,
    /// A preview's scale isn't the tiles', and it's wanted as long as its page is.
    pub(super) preview: bool,
    pub(super) generation: u64,
    pub(super) cancel: Option<CancelHandle>,
    pub(super) cancel_asked: bool,
    /// It's reading text. A selection or a copy can want a page that isn't in view, so where the view is says nothing about it.
    pub(super) text: bool,
    /// It's drawing for a hidden-text check too, which can take seconds, and can go when there's something more wanted.
    pub(super) check: bool,
}

/// What a file's renderer is to have done, at the thread that draws' next turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Renew {
    /// It has the wrong version open: it's let go of, and the next job opens what's there now.
    Reopen,
    /// This tab of it is being shown: its worker, if it has one open, says the file's length again to the tab whose own file it is. A worker says it
    /// once in its life, and a tab put to sleep and woken from its sleep file while the same worker lived (the view let go of and shown again before that
    /// thread came round, a whole-suite run of 2026-11-13; or never let go of, as a sleep and a wake fell into one look, the second review) never heard it
    /// again, so the session never checked what the tab's sleep file said and wrote it none after. Asked where the tab is shown, so it doesn't depend on how
    /// views come and go.
    TellLength(u32),
}

/// Built only in the tiles module ([`Work::new`]), as each of [`Reader`]'s parts is (see there): the engine reaches it through `Reader`'s field for what it takes and notes below.
#[derive(Debug)]
pub(crate) struct Work {
    pub(super) running: Option<Running>,
    pub(super) next_generation: u64,
    /// **What the thread that draws does to a file's renderer when it next comes round** ([`Renew`]): opens it again, as it has the wrong version open, or has
    /// its worker say the file's length again. Touched only when one of those is asked and when that thread comes round, which empties it; one entry a file
    /// and kind, so bounded by the files there are.
    pub(super) renew: Vec<(PathBuf, Renew)>,
    /// Files the renderer couldn't open for want of a password, which the app has none for: a protected file moved to another volume, or replaced, while it was open. The app asks for it again. At most [`MAX_KEYLESS`].
    pub(super) keyless: Vec<PathBuf>,
    /// The thread that draws has failed too many times to go on trying, and this says so.
    pub(super) gave_up: bool,
    /// For QA: nothing's drawn, so a first frame stays to be looked at.
    pub(super) hold_drawing: bool,
    pub(super) wanted_files: Arc<Vec<PathBuf>>,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Work>() <= 128, "what's asked of the renderer grew");

impl Work {
    pub(super) fn new() -> Self {
        Self {
            running: None,
            next_generation: 1,
            wanted_files: Arc::new(Vec::new()),
            gave_up: false,
            renew: Vec::new(),
            keyless: Vec::new(),
            hold_drawing: false,
        }
    }

    pub(super) fn fresh_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }
}

impl Work {
    /// What's to be done to renderers ([`Renew`]), since this was last asked. Asking clears it.
    pub(crate) fn take_renewals(&mut self) -> Vec<(PathBuf, Renew)> {
        std::mem::take(&mut self.renew)
    }

    /// This file's renderer is to have this done when the thread that draws next comes round, once however often it's asked.
    pub(super) fn renew(&mut self, file: Option<&PathBuf>, how: Renew) {
        if let Some(file) = file.filter(|file| !self.renew.iter().any(|(had, was)| had == *file && *was == how)) {
            self.renew.push((file.clone(), how));
        }
    }

    /// Files the renderer found need a password it has none for, since this was last asked. Asking clears them.
    pub(crate) fn take_keyless(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.keyless)
    }

    /// For QA: nothing's drawn till it's let go, so a first frame stays to be looked at.
    pub(crate) fn hold_drawing(&mut self, on: bool) {
        self.hold_drawing = on;
    }

    /// Files the engine should keep a renderer for: hot and warm tabs'.
    pub(crate) fn wanted_files(&self) -> Arc<Vec<PathBuf>> {
        Arc::clone(&self.wanted_files)
    }

    /// A job has started. Its cancel handle lets the UI thread stop it the moment it stops being wanted.
    pub(crate) fn note_started(&mut self, job: &Job, cancel: Option<CancelHandle>) {
        let (rect, scale, preview) = match &job.kind {
            JobKind::Draw { scale, rect, what, .. } => (Some(*rect), *scale, *what == DrawKind::Preview),
            _ => (None, 0.0, false),
        };
        let (text, check) = match job.kind {
            JobKind::Text { check } => (true, check),
            _ => (false, false),
        };
        self.running = Some(Running {
            tab: job.tab,
            position: job.position,
            rect,
            scale,
            preview,
            generation: job.generation,
            cancel,
            cancel_asked: false,
            text,
            check,
        });
    }

    /// What can stop the job now running, once there's something that can.
    pub(crate) fn set_cancel(&mut self, cancel: CancelHandle) {
        if let Some(running) = &mut self.running {
            running.cancel = Some(cancel);
        }
    }
}

impl Reader {
    pub(super) fn recount_wanted_files(&mut self, warm: &[u32]) {
        let mut files: Vec<PathBuf> = Vec::new();
        for tab in warm.iter().copied().chain(self.docs.hot) {
            if let Some(doc) = self.docs.views.get(&tab) {
                files.extend(doc.map.files.iter().cloned());
            }
        }
        files.sort();
        files.dedup();
        if *self.work.wanted_files != files {
            self.work.wanted_files = Arc::new(files);
        }
    }
}
