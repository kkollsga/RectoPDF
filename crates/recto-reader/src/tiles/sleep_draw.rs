//! **A tab left at a place it has no picture of gets its place drawn, before it can sleep bare** (owner, 2026-11-13, asked whether the place should be drawn first: "I think it makes sense, and extra draw is not costly right?"). A tab moved (a `goto`, a quick scroll) and left before anything was drawn there has no picture of its place in what it was left with, so its sleep file has none and it wakes to bare paper until the renderer catches up.
//!
//! **When:** as it leaves the screen, which is the one moment its worker is sure to be there. Going dormant is too late: a warm tab's worker leaves after 30 s with no job (`Policy::idle_exit`), and a tab left alone goes dormant after five minutes, so by then there's none, and drawing would mean starting one. What's drawn goes into what it was left with ([`super::doc::DocDrawing::left_pictures`], 4 MiB a tab at most), so whichever way it goes to sleep later (left alone, pushed out of the warm ones, the budget, pressure, being asked), its one sleep file has them. Going dormant never waits for any of this, and neither does anyone waiting for the view to settle ([`crate::Engine::wait_settled`]): [`SleepDraws::under_way`] says whether it's still going.
//!
//! **The rules,** decided by [`next`], a pure step, and held by its tests:
//! - Only pages at its place that owe a picture (the pages its sleep file would take and its wake would show, [`super::wake::left_place`], reckoned again when a size learned for it moves its layout) and that it was left with none of. A place already drawn asks for nothing.
//! - Each a preview, at the preview's scale: what a sleep file's picture is, and nothing bigger. A heavy page gets none, as it has no preview on screen either, and nor does one found to need a heavy worker while it's drawn; a page that can't be drawn is passed over. One that wouldn't fit under the cap isn't drawn at all.
//! - Only once the tab showing has nothing left to do and isn't waiting to be laid out or sent to its place: the person's view comes first, and a draw for this stops being wanted the moment the view wants something.
//! - One tab at a time, at most [`MAX_SLEEP_SHOTS`] pages, and [`MAX_JOBS`] jobs for them in all. A newer tab left owing takes the place of an older one, unless its own pictures are at their cap.
//! - **Never a new worker** (owner: "no file is opened again for it"): every job for this is done with [`recto_render_service::Workers::OnlyTheOneThere`], so the renderer starts none, opens nothing again, makes nothing heavy, tries nothing again in a new one and gives a worker waiting to retire nothing for another page (what's still of the page it just drew, that page's image boxes and size, it does, as it would outside the mode); a job that would have needed any of that ends the asking ([`Ended::NoWorker`]). Nor does a job for this put off a big worker's rest: it's looked at between these jobs as it is at rest, so the tab showing's worker holding a heavy page goes 3 s (`Policy::heavy_idle_exit`) after the person's last job, and at most one of these jobs (a `page_deadline`) later.
//! - **A left tab's worker lives at most `idle_exit` + 5 s + one `page_deadline` past the start of its asking:** a job for this resets the worker's own idle clock, so no job for this starts more than [`ASKING_FOR`] after the asking's first ([`Ended::OutOfTime`]), and each is held to the page deadline.
//! - It ends when the tab comes back to the screen (what's drawn is a preview like any other, and used), when it goes cold (nothing's held for a cold tab), when its pictures would pass their cap, and for [`PRESSURE_QUIET`] after the system asks for memory back, which stops a job for it that's running too.
//!
//! What's held for it is a few bytes for each page view, never one per tab: one tab's lists of at most twelve pages, and the counts.

use super::wake::{MAX_LEFT_PICTURE_BYTES, MAX_SLEEP_SHOTS, in_view_of, left_place};
use super::{DocView, DrawKind, JobPlan, MAX_PREVIEW_BYTES, PREVIEW_DIVISOR, PlanKind, Reader};
use recto_proto::{BYTES_PER_PIXEL, MAX_SCALE, MIN_SCALE, TileRect, device_size};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// After the system's asked for memory back, nothing's drawn for this for so long.
pub(super) const PRESSURE_QUIET: Duration = Duration::from_secs(60);

/// Jobs one tab's asking may take, its sizes and weights with its previews: three a page. A job that keeps coming back cancelled can't keep it going.
pub(super) const MAX_JOBS: u32 = 3 * MAX_SLEEP_SHOTS as u32;

/// No job for one tab's asking starts later than this after its first: each job keeps its worker's own idle clock from running out, and this is what that can cost.
pub(super) const ASKING_FOR: Duration = Duration::from_secs(5);

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
    /// Its file's worker had gone, or was waiting to retire, or the job would have needed another: none is started for this.
    NoWorker,
    /// Another tab left owing took its place.
    Replaced,
    /// Its pictures are at their cap, or the next wouldn't fit under it.
    Full,
    /// It took [`MAX_JOBS`].
    OutOfJobs,
    /// [`ASKING_FOR`] had passed since its first job.
    OutOfTime,
    /// A page its map has no source for, so nothing at its place can be said to be of its file.
    NoSource,
}

impl Ended {
    const ALL: [Self; 10] = [
        Self::Done,
        Self::Hot,
        Self::Cold,
        Self::Pressure,
        Self::NoWorker,
        Self::Replaced,
        Self::Full,
        Self::OutOfJobs,
        Self::OutOfTime,
        Self::NoSource,
    ];

    /// Its word in `qa state`.
    fn word(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Hot => "hot",
            Self::Cold => "cold",
            Self::Pressure => "pressure",
            Self::NoWorker => "no_worker",
            Self::Replaced => "replaced",
            Self::Full => "full",
            Self::OutOfJobs => "out_of_jobs",
            Self::OutOfTime => "out_of_time",
            Self::NoSource => "no_source",
        }
    }
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
pub(super) struct Owed {
    pub(super) tab: u32,
    pub(super) pages: Vec<u32>,
    /// Pages passed over as too large for a preview, or found to need a heavy worker: not asked for again when its place is reckoned again.
    pub(super) passed: Vec<u32>,
    pub(super) jobs: u32,
    /// How tall its layout was when its pages were reckoned. A size learned since moves the place, and they're reckoned again.
    pub(super) laid_out: f64,
    /// When its first job started.
    pub(super) began: Option<Instant>,
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
    /// Pages passed over because drawing them would have taken a heavy worker.
    pub heavy_passed: u64,
    /// How many askings ended each way, in [`Ended::ALL`]'s order. A tab left owing with its pictures already at their cap counts as ended full.
    ended: [u64; Ended::ALL.len()],
}

/// **The page view's one asking,** kept with its wake.
#[derive(Debug, Default)]
pub struct SleepDraws {
    pub(super) owed: Option<Owed>,
    quiet_until: Option<Instant>,
    counts: SleepDrawCounts,
}

/// Bytes of pictures a tab was left with.
fn picture_bytes(doc: &DocView) -> usize {
    doc.drawing
        .left_pictures
        .as_ref()
        .map_or(0, |pictures| pictures.iter().map(|picture| picture.bgra.len()).sum())
}

/// The pages at a tab's place it has no picture of, but for those passed over.
// R3.6: a `Reader` operation the ratchet's method count can't see, as a free function taking the whole page view; so are `preview_of` and the associated functions of `SleepDraws` that take `reader` below.
fn owed_pages(reader: &Reader, doc: &DocView, passed: &[u32]) -> Vec<u32> {
    let (pages, from, height) = left_place(reader, doc);
    let have = doc.drawing.left_pictures.as_deref().unwrap_or_default();
    in_view_of(doc, &pages, from, height)
        .into_iter()
        .filter(|page| !passed.contains(page) && !have.iter().any(|picture| picture.page == *page))
        .collect()
}

/// A page's preview, as [`SleepDraws::plan`] asks for it: its scale and size, or `None` if it's too large for a preview to be cheap.
fn preview_of(reader: &Reader, doc: &DocView, page: u32) -> Option<(f32, u32, u32)> {
    let scale = ((doc.layout.fit() * reader.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
    let size = doc.layout.raw_size(page);
    let (width, height) = device_size(size.width, size.height, scale).ok()?;
    (preview_bytes(width, height) <= MAX_PREVIEW_BYTES).then_some((scale, width, height))
}

fn preview_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL)
}

/// Whether a picture of this size fits beside what's held, under the cap.
fn fits(held: usize, width: u32, height: u32) -> bool {
    held as u64 + preview_bytes(width, height) <= MAX_LEFT_PICTURE_BYTES as u64
}

// R3.6: every associated function here that takes `reader: &Reader` or `&mut Reader` (`left`, `view_waits`, `plan`, `is_for`, `wants`, `unstarted`,
// `drawn`, `under_way`, `report`) is a `Reader` operation the ratchet's method count can't see: it takes the whole page view, not this part, and reaches
// every field `Reader`'s own methods do. Its decisions are `next`'s, a pure step tested without the page view; what's here gathers what `next` is given.
impl SleepDraws {
    /// **A tab's just left the screen,** with what it showed taken as its pictures and its place noted: the pages there it has no picture of are asked for. Nothing for a tab with no version said (its sleep file can't have pictures), or under pressure, and one whose pictures are at their cap takes nothing's place.
    pub(super) fn left(reader: &mut Reader, tab: u32) {
        let Some(doc) = reader.docs.views.get(&tab) else { return };
        let versioned = doc.map.files.first().and_then(|file| doc.drawing.versions.get(file)).is_some();
        if reader.wake.draws.quiet() || !versioned {
            return;
        }
        let owed = owed_pages(reader, doc, &[]);
        let (held, laid_out) = (picture_bytes(doc), doc.layout.document_height());
        // Room for its first page's picture, as near as can be told before its size is.
        let room = owed
            .first()
            .is_none_or(|page| preview_of(reader, doc, *page).is_none_or(|(_, width, height)| fits(held, width, height)));
        let draws = &mut reader.wake.draws;
        if owed.is_empty() {
            draws.counts.leaves_pictured += 1;
            return;
        }
        draws.counts.leaves_owing += 1;
        draws.counts.pages_owed += owed.len() as u64;
        if !room {
            draws.counts.ended[Ended::Full as usize] += 1;
            return;
        }
        if draws.owed.as_ref().is_some_and(|older| older.tab != tab) {
            draws.end(Ended::Replaced);
        }
        draws.owed = Some(Owed {
            tab,
            pages: owed,
            passed: Vec::new(),
            jobs: 0,
            laid_out,
            began: None,
        });
    }

    /// Whether the person's view is waiting for something before anything's drawn for a tab off screen: a jump not arrived, a view with no height, a tab showing not laid out yet, or what it wants.
    fn view_waits(reader: &Reader) -> bool {
        let unlaid = reader
            .docs
            .hot
            .and_then(|tab| reader.docs.views.get(&tab))
            .is_some_and(|doc| doc.drawing.problem.is_none() && doc.drawing.tile_scale <= 0.0);
        reader.place.jump.is_some() || reader.viewport.height <= 0.0 || unlaid || reader.view_needs_something()
    }

    /// **The next job for the tab owed pictures,** asked only once the tab showing has nothing to do. `None` when there's nothing, and the asking's ended if that's why.
    pub(crate) fn plan(reader: &mut Reader) -> Option<JobPlan> {
        if !reader.window_open || reader.work.hold_drawing || Self::view_waits(reader) {
            return None;
        }
        let mut owed = reader.wake.draws.owed.take()?;
        let doc = reader.docs.views.get(&owed.tab);
        if let Some(doc) = doc
            && doc.layout.document_height() != owed.laid_out
        {
            owed.pages = owed_pages(reader, doc, &owed.passed);
            owed.laid_out = doc.layout.document_height();
        }
        let tab = TabNow {
            held: doc.is_some(),
            hot: reader.docs.hot == Some(owed.tab),
            problem: doc.is_some_and(|doc| doc.drawing.problem.is_some()),
            pressure: reader.wake.draws.quiet(),
            picture_bytes: doc.map_or(0, picture_bytes),
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
        let step = match step {
            Next::End(_) => step,
            _ if owed.began.is_some_and(|began| began.elapsed() > ASKING_FOR) => Next::End(Ended::OutOfTime),
            _ => step,
        };
        let (page, kind) = match step {
            Next::End(why) => {
                reader.wake.draws.end(why);
                return None;
            }
            Next::Size(page) => (page, PlanKind::Size),
            Next::Weight(page) => (page, PlanKind::Traits),
            Next::Preview(page) => {
                // Held, or `next` would have ended it; should that ever not be so, it ends here and is counted.
                let Some(doc) = doc else {
                    reader.wake.draws.end(Ended::Cold);
                    return None;
                };
                let Some((scale, width, height)) = preview_of(reader, doc, page) else {
                    // Too large for a preview to be cheap: passed over, and the next looked at.
                    owed.pages.retain(|owed| *owed != page);
                    owed.passed.push(page);
                    reader.wake.draws.owed = Some(owed);
                    return Self::plan(reader);
                };
                // Not drawn to be thrown away: what wouldn't fit under the cap isn't asked for.
                if !fits(tab.picture_bytes, width, height) {
                    reader.wake.draws.end(Ended::Full);
                    return None;
                }
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
        let Some((file, source_page)) = doc.and_then(|doc| doc.map.source(page)) else {
            reader.wake.draws.end(Ended::NoSource);
            return None;
        };
        owed.jobs += 1;
        owed.began.get_or_insert_with(Instant::now);
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
        self.counts.ended[why as usize] += 1;
    }

    fn quiet(&self) -> bool {
        self.quiet_until.is_some_and(|until| Instant::now() < until)
    }

    /// Whether a job for this tab is one asked here: it's owed pictures, and isn't the tab showing.
    pub(crate) fn is_for(reader: &Reader, tab: u32) -> bool {
        reader.docs.hot != Some(tab) && reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.tab == tab)
    }

    /// Whether what's running, for this tab and page, is still wanted for this: the person's view is waiting for nothing meanwhile.
    pub(super) fn wants(reader: &Reader, tab: u32, page: u32) -> bool {
        Self::is_for(reader, tab) && reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.pages.contains(&page)) && !Self::view_waits(reader)
    }

    /// **A job for this wasn't done, as it would have taken another worker** (a heavy one, if `heavy`): a page that needs a heavy one is passed over, and with no worker to do the rest, the asking ends.
    pub(super) fn unstarted(reader: &mut Reader, tab: u32, page: u32, heavy: bool) {
        if !Self::is_for(reader, tab) {
            return;
        }
        let draws = &mut reader.wake.draws;
        if !heavy {
            draws.end(Ended::NoWorker);
            return;
        }
        draws.counts.heavy_passed += 1;
        if let Some(owed) = &mut draws.owed {
            owed.pages.retain(|owed| *owed != page);
            owed.passed.push(page);
        }
    }

    /// **A preview's been drawn and kept, as `id`:** if it's one asked here, a copy goes into what the tab was left with, under that's cap, and its page is done with whichever way it went. Kept for a tab off screen, the copy in the cache stays, so a return while the tab's still warm shows it, but as the oldest preview there, whether it went into what the tab was left with or not (its asking over as it landed, its copy not made): it's the first to go when there are too many, never one of the tab showing's.
    pub(super) fn drawn(reader: &mut Reader, tab: u32, page: u32, id: u64) {
        if reader.docs.hot == Some(tab) {
            return;
        }
        if let Some(entry) = reader.cache.entries.get_mut(&id) {
            entry.last_used = 0;
        }
        if !Self::is_for(reader, tab) || !reader.wake.draws.owed.as_ref().is_some_and(|owed| owed.pages.contains(&page)) {
            return;
        }
        if let Some(owed) = &mut reader.wake.draws.owed {
            owed.pages.retain(|owed| *owed != page);
        }
        let Some(picture) = reader.copy_preview(id) else { return };
        let Some(doc) = reader.docs.views.get_mut(&tab) else { return };
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

    /// The system's asked for memory back: nothing's drawn for this from now till [`PRESSURE_QUIET`] has passed. The caller stops a job for it that's running.
    pub(super) fn pressure(&mut self) {
        if self.owed.is_some() {
            self.end(Ended::Pressure);
        }
        self.quiet_until = Some(Instant::now() + PRESSURE_QUIET);
    }

    /// Whether there's more to draw for this that can be drawn: for a test or QA that waits for it, as nothing that waits for the view does. Not once the thread that draws has given up.
    pub(crate) fn under_way(reader: &Reader) -> bool {
        reader.window_open && !reader.work.hold_drawing && !reader.work.gave_up && reader.wake.draws.owed.is_some()
    }

    pub(super) fn report(reader: &Reader) -> Value {
        let draws = &reader.wake.draws;
        let counts = draws.counts;
        let ended: Map<String, Value> = Ended::ALL
            .iter()
            .map(|why| (why.word().to_string(), json!(counts.ended[*why as usize])))
            .collect();
        json!({
            "under_way": Self::under_way(reader),
            "owed_tab": draws.owed.as_ref().map(|owed| owed.tab),
            "owed_pages": draws.owed.as_ref().map_or(0, |owed| owed.pages.len()),
            "leaves_owing": counts.leaves_owing,
            "leaves_pictured": counts.leaves_pictured,
            "pages_owed": counts.pages_owed,
            "jobs": counts.jobs,
            "pictures": counts.pictures,
            "picture_bytes": counts.picture_bytes,
            "heavy_passed": counts.heavy_passed,
            "ended": ended,
        })
    }
}
