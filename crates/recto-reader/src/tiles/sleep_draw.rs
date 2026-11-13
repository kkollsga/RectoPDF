//! **A tab left at a place it has no picture of gets its place drawn, before it can sleep bare** (owner, 2026-11-13, asked whether the place should be drawn first: "I think it makes sense, and extra draw is not costly right?"). A tab moved (a `goto`, a quick scroll) and left before anything was drawn there has no picture of its place in what it was left with, so its sleep file has none and it wakes to bare paper until the renderer catches up.
//!
//! **When:** as it leaves the screen, which is the one moment its worker is sure to be there. Going dormant is too late: a warm tab's worker leaves after 30 s with no job (`Policy::idle_exit`), and a tab left alone goes dormant after five minutes, so by then there's none, and drawing would mean starting one. What's drawn goes into what it was left with ([`super::doc::DocDrawing::left_pictures`], 4 MiB a tab at most), so whichever way it goes to sleep later (left alone, pushed out of the warm ones, the budget, pressure, being asked), its one sleep file has them. Going dormant never waits for any of this.
//!
//! **The rules,** decided by [`next`], a pure step, and held by its tests:
//! - Only pages at its place that owe a picture (the pages its sleep file would take, [`super::wake::left_place`]) and that it was left with none of. A place already drawn asks for nothing.
//! - Each a preview, at the preview's scale: what a sleep file's picture is, and nothing bigger. A heavy page gets none, as it has no preview on screen either; a page that can't be drawn is passed over.
//! - Only once the tab showing has nothing left to do: the person's view comes first, and a draw for this stops being wanted the moment the view wants something.
//! - One tab at a time, at most [`MAX_SLEEP_SHOTS`] pages, and [`MAX_JOBS`] jobs for them in all. A newer tab left owing takes the place of an older one.
//! - Never a new worker: a job for this finds its file's worker there, or the asking ends ([`Ended::NoWorker`]).
//! - It ends when the tab comes back to the screen (what's drawn is a preview like any other, and used), when it goes cold (nothing's held for a cold tab), when its pictures would pass their cap, and for [`PRESSURE_QUIET`] after the system asks for memory back.
//!
//! What's held for it is a few bytes for each page view, never one per tab: one tab's list of at most twelve pages, and the counts.

use super::wake::{MAX_LEFT_PICTURE_BYTES, MAX_SLEEP_SHOTS, in_view_of, left_place};
use super::{DrawKind, JobPlan, MAX_PREVIEW_BYTES, PREVIEW_DIVISOR, PlanKind, Reader};
use recto_proto::{BYTES_PER_PIXEL, MAX_SCALE, MIN_SCALE, TileRect, device_size};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// After the system's asked for memory back, nothing's drawn for this for so long.
pub(super) const PRESSURE_QUIET: Duration = Duration::from_secs(60);

/// Jobs one tab's asking may take, its sizes and weights with its previews: three a page. A job that keeps coming back cancelled can't keep it going.
pub(super) const MAX_JOBS: u32 = 3 * MAX_SLEEP_SHOTS as u32;

/// What's known of a page owed a picture, as [`next`] looks at it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PageNow {
    /// It has a picture in what the tab was left with.
    pub(super) has_picture: bool,
    /// Its size is known.
    pub(super) sized: bool,
    /// Whether it's heavy, if that's been asked.
    pub(super) heavy: Option<bool>,
    /// It can't be drawn, or not now.
    pub(super) issue: bool,
}

/// What the tab owed pictures is to the page view now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TabNow {
    /// The page view still holds it: it's warm.
    pub(super) held: bool,
    /// It's the tab showing.
    pub(super) hot: bool,
    /// The whole file can't be drawn.
    pub(super) problem: bool,
    /// The system's asked for memory back lately.
    pub(super) pressure: bool,
    /// What it was left with holds this many bytes of pictures.
    pub(super) picture_bytes: usize,
}

/// Why a tab's asking ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ended {
    /// Every page it was owed has its picture, or was passed over.
    Done,
    /// It came back to the screen.
    Hot,
    /// It went cold, or can't be drawn at all.
    Cold,
    Pressure,
    /// Its file's worker had gone, and none is started for this.
    NoWorker,
    /// Another tab left owing took its place.
    Replaced,
    /// Its pictures are at their cap.
    Full,
    /// It took [`MAX_JOBS`].
    OutOfJobs,
}

/// What's next for the tab owed pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    Size(u32),
    Weight(u32),
    Preview(u32),
    End(Ended),
}

/// **The one step:** what's next for a tab owed pictures of these pages, from what's known of it and of each. Pure. Pages that need nothing more (a picture there, heavy, or not drawable) are taken off the front as it goes.
pub(super) fn next(tab: TabNow, pages: &mut Vec<u32>, jobs: u32, page_now: impl Fn(u32) -> PageNow) -> Next {
    if !tab.held || tab.problem {
        return Next::End(Ended::Cold);
    }
    if tab.hot {
        return Next::End(Ended::Hot);
    }
    if tab.pressure {
        return Next::End(Ended::Pressure);
    }
    if tab.picture_bytes >= MAX_LEFT_PICTURE_BYTES {
        return Next::End(Ended::Full);
    }
    while let Some(&page) = pages.first() {
        let now = page_now(page);
        if now.has_picture || now.issue || now.heavy == Some(true) {
            pages.remove(0);
            continue;
        }
        if jobs >= MAX_JOBS {
            return Next::End(Ended::OutOfJobs);
        }
        return if !now.sized {
            Next::Size(page)
        } else if now.heavy.is_none() {
            Next::Weight(page)
        } else {
            Next::Preview(page)
        };
    }
    Next::End(Ended::Done)
}

/// One tab's pages owed pictures.
#[derive(Debug)]
struct Owed {
    tab: u32,
    pages: Vec<u32>,
    jobs: u32,
}

/// What's been counted, for QA and the owner's question of what the extra draw costs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SleepDrawCounts {
    /// Tabs left at a place they had no picture of, with pages owed.
    pub leaves_owing: u64,
    /// Tabs left with every page at their place pictured, which ask for nothing.
    pub leaves_pictured: u64,
    pub pages_owed: u64,
    /// Jobs asked for this: sizes, weights and previews.
    pub jobs: u64,
    /// Pictures that went into what a tab was left with.
    pub pictures: u64,
    pub picture_bytes: u64,
    pub done: u64,
    pub ended_hot: u64,
    pub ended_cold: u64,
    pub ended_pressure: u64,
    pub no_worker: u64,
    pub replaced: u64,
    pub full: u64,
    pub out_of_jobs: u64,
}

/// **The page view's one asking,** kept with its wake.
#[derive(Debug, Default)]
pub struct SleepDraws {
    owed: Option<Owed>,
    quiet_until: Option<Instant>,
    counts: SleepDrawCounts,
}

impl SleepDraws {
    /// **A tab's just left the screen,** with what it showed taken as its pictures and its place noted: the pages there it has no picture of are asked for. Nothing for a tab with no version said (its sleep file can't have pictures), or under pressure.
    pub(super) fn left(reader: &mut Reader, tab: u32) {
        let Some(doc) = reader.docs.get(&tab) else { return };
        let pressure = reader.wake.draws.quiet_until.is_some_and(|until| Instant::now() < until);
        let versioned = doc.map.files.first().and_then(|file| doc.drawing.versions.get(file)).is_some();
        if pressure || !versioned {
            return;
        }
        let (pages, from, height) = left_place(reader, doc);
        let have = doc.drawing.left_pictures.as_deref().unwrap_or_default();
        let owed: Vec<u32> = in_view_of(doc, &pages, from, height)
            .into_iter()
            .filter(|page| !have.iter().any(|picture| picture.page == *page))
            .collect();
        let draws = &mut reader.wake.draws;
        if owed.is_empty() {
            draws.counts.leaves_pictured += 1;
            return;
        }
        if draws.owed.as_ref().is_some_and(|older| older.tab != tab) {
            draws.end(Ended::Replaced);
        }
        draws.counts.leaves_owing += 1;
        draws.counts.pages_owed += owed.len() as u64;
        draws.owed = Some(Owed { tab, pages: owed, jobs: 0 });
    }

    /// **The next job for the tab owed pictures,** asked only once the tab showing has nothing to do. `None` when there's nothing, and the asking's ended if that's why.
    pub(crate) fn plan(reader: &mut Reader) -> Option<JobPlan> {
        if !reader.window_open || reader.work.hold_drawing {
            return None;
        }
        let mut owed = reader.wake.draws.owed.take()?;
        let pressure = reader.wake.draws.quiet_until.is_some_and(|until| Instant::now() < until);
        let doc = reader.docs.get(&owed.tab);
        let tab = TabNow {
            held: doc.is_some(),
            hot: reader.hot == Some(owed.tab),
            problem: doc.is_some_and(|doc| doc.drawing.problem.is_some()),
            pressure,
            picture_bytes: doc
                .and_then(|doc| doc.drawing.left_pictures.as_ref())
                .map_or(0, |pictures| pictures.iter().map(|picture| picture.bgra.len()).sum()),
        };
        let step = next(tab, &mut owed.pages, owed.jobs, |page| {
            let Some(doc) = doc else { return PageNow::default() };
            PageNow {
                has_picture: doc
                    .drawing
                    .left_pictures
                    .as_ref()
                    .is_some_and(|pictures| pictures.iter().any(|picture| picture.page == page)),
                sized: doc.layout.is_known(page),
                heavy: doc.drawing.heavy.get(&page).copied(),
                issue: page >= doc.layout.count() || doc.drawing.issues.contains_key(&page),
            }
        });
        let (page, kind) = match step {
            Next::End(why) => {
                reader.wake.draws.end(why);
                return None;
            }
            Next::Size(page) => (page, PlanKind::Size),
            Next::Weight(page) => (page, PlanKind::Traits),
            Next::Preview(page) => {
                let doc = doc?;
                let scale = ((doc.layout.fit() * reader.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
                let size = doc.layout.raw_size(page);
                let fits = device_size(size.width, size.height, scale)
                    .ok()
                    .filter(|(width, height)| u64::from(*width) * u64::from(*height) * u64::from(BYTES_PER_PIXEL) <= MAX_PREVIEW_BYTES);
                let Some((width, height)) = fits else {
                    // Too large for a preview to be cheap: passed over, and the next looked at next time round.
                    owed.pages.retain(|owed| *owed != page);
                    reader.wake.draws.owed = Some(owed);
                    return Self::plan(reader);
                };
                (
                    page,
                    PlanKind::Draw {
                        scale,
                        rect: TileRect { x: 0, y: 0, width, height },
                        what: DrawKind::Preview,
                        paper: reader.paper,
                    },
                )
            }
        };
        // A page its map has no source for can't be drawn: the asking ends there, said, rather than gone with no word.
        let Some((file, source_page)) = doc.and_then(|doc| doc.map.source(page)) else {
            reader.wake.draws.end(Ended::Cold);
            return None;
        };
        owed.jobs += 1;
        let tab = owed.tab;
        reader.wake.draws.counts.jobs += 1;
        reader.wake.draws.owed = Some(owed);
        let generation = reader.work.fresh_generation();
        Some(JobPlan {
            tab,
            position: page,
            file,
            source_page,
            generation,
            kind,
        })
    }

    fn end(&mut self, why: Ended) {
        self.owed = None;
        let counter = match why {
            Ended::Done => &mut self.counts.done,
            Ended::Hot => &mut self.counts.ended_hot,
            Ended::Cold => &mut self.counts.ended_cold,
            Ended::Pressure => &mut self.counts.ended_pressure,
            Ended::NoWorker => &mut self.counts.no_worker,
            Ended::Replaced => &mut self.counts.replaced,
            Ended::Full => &mut self.counts.full,
            Ended::OutOfJobs => &mut self.counts.out_of_jobs,
        };
        *counter += 1;
    }

    /// Whether a job for this tab is one asked here: it's owed pictures, and isn't the tab showing.
    pub(crate) fn is_for(reader: &Reader, tab: u32) -> bool {
        reader.hot != Some(tab) && reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.tab == tab)
    }

    /// Whether what's running, for this tab and page, is still wanted for this: the person's view wants nothing meanwhile.
    pub(super) fn wants(reader: &Reader, tab: u32, page: u32) -> bool {
        Self::is_for(reader, tab) && reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.pages.contains(&page)) && !reader.view_needs_something()
    }

    /// The job's file has no worker: none's started for this, and the asking ends.
    pub(crate) fn no_worker(reader: &mut Reader) {
        reader.wake.draws.end(Ended::NoWorker);
    }

    /// **A preview's been drawn and kept, as `id`:** if it's one asked here, a copy goes into what the tab was left with, under that's cap, and its page is done with whichever way it went.
    pub(super) fn drawn(reader: &mut Reader, tab: u32, page: u32, id: u64) {
        if !Self::is_for(reader, tab) || !reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.pages.contains(&page)) {
            return;
        }
        if let Some(owed) = &mut reader.wake.draws.owed {
            owed.pages.retain(|owed| *owed != page);
        }
        let Some(picture) = reader.copy_preview(id) else { return };
        let Some(doc) = reader.docs.get_mut(&tab) else { return };
        let left = doc.drawing.left_pictures.get_or_insert_with(Vec::new);
        let held: usize = left.iter().map(|picture| picture.bgra.len()).sum();
        if held + picture.bgra.len() > MAX_LEFT_PICTURE_BYTES {
            reader.wake.draws.end(Ended::Full);
            return;
        }
        let bytes = picture.bgra.len() as u64;
        left.push(picture);
        reader.wake.draws.counts.pictures += 1;
        reader.wake.draws.counts.picture_bytes += bytes;
    }

    /// The system's asked for memory back: nothing's drawn for this from now till [`PRESSURE_QUIET`] has passed.
    pub(super) fn pressure(&mut self) {
        if self.owed.is_some() {
            self.end(Ended::Pressure);
        }
        self.quiet_until = Some(Instant::now() + PRESSURE_QUIET);
    }

    /// Whether there's more to draw for this that can be drawn, for whoever's waiting for everything to settle.
    pub(crate) fn under_way(reader: &Reader) -> bool {
        reader.window_open && !reader.work.hold_drawing && reader.wake.draws.owed.is_some()
    }

    pub(super) fn report(&self) -> Value {
        let counts = self.counts;
        json!({
            "owed_tab": self.owed.as_ref().map(|owed| owed.tab),
            "owed_pages": self.owed.as_ref().map_or(0, |owed| owed.pages.len()),
            "leaves_owing": counts.leaves_owing,
            "leaves_pictured": counts.leaves_pictured,
            "pages_owed": counts.pages_owed,
            "jobs": counts.jobs,
            "pictures": counts.pictures,
            "picture_bytes": counts.picture_bytes,
            "ended": {
                "done": counts.done,
                "hot": counts.ended_hot,
                "cold": counts.ended_cold,
                "pressure": counts.ended_pressure,
                "no_worker": counts.no_worker,
                "replaced": counts.replaced,
                "full": counts.full,
                "out_of_jobs": counts.out_of_jobs,
            },
        })
    }
}
