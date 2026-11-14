//! **The thread that draws:** what it does with each job the page view asks for, outside the page view's lock but for short looks, and how it comes back
//! from a panic. Free functions over the engine's shared state: nothing here is a method, and the one lock order (the page view's, then the pending
//! place's) is the engine's own.

use crate::layout::PageBox;
use crate::paper::{DarkTile, PaperKind, dark_rgb, dim_page, dim_rgb, is_grey, turn_over};
use crate::tiles::sleep_draw::SleepDraws;
use crate::tiles::{DrawKind, Job, JobKind, JobPlan, Outcome, Renew};
use recto_index::select::PageText;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig, TileKind, TileRequest, TileResult, Workers};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use super::look::{Looks, tile_look};
use super::{Changed, Shared, lock, note_change, selecting_news, signal_ops, take_pending_follow, tell_selecting};

/// Files that can be being opened at once. Past this, more that aren't answering wait for one that is.
const MAX_OPENINGS: usize = 4;
/// How long a job waits for its file to open before the page says so. It stays being tried.
const OPEN_PATIENCE: Duration = Duration::from_secs(3);
const OPEN_LOOK: Duration = Duration::from_millis(20);
/// A page from a PDF added to this one, which has changed since: its page numbers are about the version it was, so what's there now isn't drawn as it.
const ADDED_FILE_CHANGED: &str = "This page comes from a PDF that has changed or moved since it was added, so it isn't drawn.";
/// A page from a PDF added to this one that needs a password: see [`added_needs_password`].
const ADDED_FILE_NEEDS_PASSWORD: &str = "This page comes from a PDF that needs a password, so it isn't shown here. Open that PDF to see it.";
/// What a page says while its file doesn't answer.
const NOT_ANSWERING: &str = "This file isn't answering. Its disk may have gone to sleep or been disconnected.";

/// Panics of the thread that draws that it comes back from, close together. Past this it would only panic again.
const MAX_PANICS_IN_A_ROW: u32 = 4;
/// A panic this long after the last one is a new story.
const PANICS_FORGOTTEN_AFTER: Duration = Duration::from_secs(60);

/// Goes round [`run`] again if it panics, so a panic can't leave pages silently undrawn with nothing left to draw them. What's held in it, workers included, goes with it, and pages are asked for again. A few in a row means trying again won't help, and the page says so.
pub(super) fn run_guarded(shared: &Shared) {
    let mut panics_in_a_row = 0;
    let mut last_panic: Option<Instant> = None;
    loop {
        // Everything shared is behind locks that don't care about poison, and is brought back into line below.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(shared))).is_ok() {
            shared.thread_ended();
            return;
        }
        panics_in_a_row = if last_panic.is_some_and(|last| last.elapsed() < PANICS_FORGOTTEN_AFTER) {
            panics_in_a_row + 1
        } else {
            1
        };
        last_panic = Some(Instant::now());
        let give_up = panics_in_a_row >= MAX_PANICS_IN_A_ROW;
        let mut reader = lock(&shared.reader);
        reader.thread_restarted(give_up);
        let has_ops = reader.host.has_ops();
        let note = reader.note().map(str::to_string);
        let selecting = selecting_news(&mut reader);
        // Told with the view let go: whoever waits for it settled waits for this too, until the thread takes it again or has gone.
        reader.work.set_telling(!give_up);
        drop(reader);
        note_change(shared, note);
        tell_selecting(shared, selecting);
        *lock(&shared.worker_pids) = Vec::new();
        shared.walled_workers.store(0, Ordering::Relaxed);
        shared.settled.notify_all();
        if has_ops {
            signal_ops(shared);
        }
        if give_up {
            shared.thread_ended();
            return;
        }
    }
}

/// **Nothing to start:** at rest as ever, or, when there was a plan whose surface the system refused, a wait before it's asked again, only while it's planned
/// (`SurfaceRetry`, in `tiles/jobs.rs`). Workers are let go in the wait as at rest: memory pressure and a refused surface come together.
fn rest_or_wait_for_a_surface(
    planned: bool,
    seen: u64,
    shared: &Shared,
    renderers: &mut HashMap<PathBuf, OpenDoc>,
    policy: &Policy,
    resting_since: &mut Option<Instant>,
) {
    let retry = {
        let mut reader = lock(&shared.reader);
        // What this turn told is told: the view taken again. Resting is said under the view's lock, which whoever waits for it to rest looks under
        // (`Qa::wait_resting`), and told as it rests ([`rest`]), so the word can't fall between their look and their wait.
        all_told(shared, &mut reader);
        shared.rests(seen);
        if planned {
            Some(reader.work.surfaces.refused())
        } else {
            reader.work.surfaces.clear();
            None
        }
    };
    rest(shared, renderers, policy, resting_since, retry);
}

fn run(shared: &Shared) {
    let mut renderers: HashMap<PathBuf, OpenDoc> = HashMap::new();
    let mut wanted = lock(&shared.reader).work.wanted_files();
    let service = shared.config.service.clone().map_or_else(ServiceConfig::discover, Ok);
    let policy = shared.config.policy.clone().unwrap_or_default();
    let mut resting_since: Option<Instant> = None;
    let mut openings: HashMap<PathBuf, Receiver<Result<File, RenderFailure>>> = HashMap::new();
    loop {
        if shared.stopping.load(Ordering::SeqCst) {
            // Dropping them kills each worker and closes its file.
            renderers.clear();
            publish_workers(shared, &mut renderers);
            // Nothing more is told: whoever waits for the view isn't left waiting on the last turn's word.
            all_told(shared, &mut lock(&shared.reader));
            shared.settled.notify_all();
            return;
        }
        let seen = shared.round_begins();
        let (plan, changed) = {
            let mut reader = lock(&shared.reader);
            // What the last turn told is told: the view taken again.
            all_told(shared, &mut reader);
            let files = reader.work.wanted_files();
            if !Arc::ptr_eq(&files, &wanted) {
                wanted = files;
            }
            let told = renew(&mut reader, &mut renderers, &mut openings);
            // Said with the page view's lock let go, as a job's saying is.
            if !told.is_empty() {
                reader.work.set_telling(true);
                drop(reader);
                for (tab, pages, version) in told {
                    shared.events.file_length(tab, pages, version);
                }
                reader = lock(&shared.reader);
                all_told(shared, &mut reader);
            }
            // What the UI thread left is told of as any way of applying it tells it: the selection's notice gone with a selection another pane took, say.
            let changed = take_pending_follow(shared, &mut reader).then(|| Changed::of(&mut reader));
            reader.work.set_telling(changed.is_some());
            (reader.next_plan(), changed)
        };
        if let Some(changed) = changed {
            changed.tell(shared);
        }
        // Its surface is made with the lock let go, which the UI thread needs on every scroll.
        let planned = plan.is_some();
        let job = plan.and_then(JobPlan::ready);
        let_go_of_unwanted(shared, &mut renderers, &wanted);
        openings.retain(|path, _| wanted.contains(path));
        publish_workers(shared, &mut renderers);
        let Some(job) = job else {
            rest_or_wait_for_a_surface(planned, seen, shared, &mut renderers, &policy, &mut resting_since);
            continue;
        };
        // **A job for a tab off screen's place ([`SleepDraws`]) starts no worker:** it's done by the one its file has, or not at all. Nor does it put off a big worker's rest, which goes on between its jobs as it does with none.
        let workers = if SleepDraws::is_for(&lock(&shared.reader), job.tab) {
            if big_workers_rest(&mut renderers, &policy, &mut resting_since).is_some_and(|left| left.is_zero()) {
                publish_workers(shared, &mut renderers);
            }
            Workers::OnlyTheOneThere
        } else {
            resting_since = None;
            Workers::MayStart
        };
        {
            let mut reader = lock(&shared.reader);
            all_told(shared, &mut reader);
            reader.work.note_started(&job, None);
        }
        if matches!(job.kind, JobKind::Links) {
            read_links(shared, &renderers, job);
            continue;
        }
        let mut file_changed = false;
        let (added_changed, own) = added_file_now(shared, &job);
        let outcome = if added_changed {
            match job.kind {
                JobKind::Text { .. } => Outcome::TextFailed,
                _ => Outcome::PageFailed {
                    message: ADDED_FILE_CHANGED.to_string(),
                    may_pass: false,
                },
            }
        } else {
            match renderer_for(shared, &mut renderers, &mut openings, job.tab, &job.file, &service, &policy) {
                // Not the tab the password was typed for: as though it had none, so its own tab is asked for it and an added page says why it isn't shown.
                Ok(open) if !open.serves(job.tab, own) => document_failed(RenderFailure::NeedsPassword),
                Ok(open) => {
                    lock(&shared.reader).work.set_cancel(open.doc.cancel_handle());
                    let outcome = DocRenderer::with_workers(open, |open| &mut open.doc, workers, |open| do_job(open, &job));
                    file_changed = seen_replaced(open) | std::mem::take(&mut open.replaced);
                    if file_changed {
                        open.looks.clear();
                        open.length_told = false;
                        shared.events.file_replaced(job.tab);
                    }
                    // Which version a worker has, before what it drew is kept, so that's what it's kept as.
                    if let Some(version) = open.doc.known_version() {
                        lock(&shared.reader).note_version(&job.file, version);
                    }
                    // Said only to the tab whose own file it is, and marked said only then: a job for a tab with this file's pages added would otherwise use
                    // up the saying, and the owner, woken from its sleep file, stay unchecked for good.
                    if !open.length_told
                        && let (Some(pages), Some(version)) = (open.doc.known_page_count(), open.doc.known_version())
                        && lock(&shared.reader).docs.is_own_file(job.tab, &job.file)
                    {
                        open.length_told = true;
                        shared.events.file_length(job.tab, pages, version);
                    }
                    outcome
                }
                Err(Unopened::Failed(failure)) => document_failed(failure),
                Err(Unopened::NotWanted) => Outcome::Cancelled,
                Err(Unopened::NotAnswering) => Outcome::PageFailed {
                    message: NOT_ANSWERING.to_string(),
                    may_pass: true,
                },
            }
        };
        let outcome = if own { outcome } else { as_an_added_files(outcome, &job) };
        // A renderer this job opened with a password for another tab's file goes now, not when the thread next comes round, which may be a long rest.
        let_go_of_unwanted(shared, &mut renderers, &wanted);
        publish_workers(shared, &mut renderers);
        job_ended(shared, job, outcome, file_changed);
    }
}

/// **What the thread told of its last turn is told,** said with the view's lock held: whoever waits for the view settled is woken, as it may be settled
/// now, and the thread may go straight on to work that settles nothing in view (drawing for a tab off screen's place), which tells nobody till it ends.
pub(super) fn all_told(shared: &Shared, reader: &mut crate::tiles::Reader) {
    if reader.work.set_telling(false) {
        shared.settled.notify_all();
    }
}

/// **A job's end:** taken into the view under its lock (a file found changed first, and a text job's read dropped with it), then told with the lock let go:
/// the note, the selection's notice, the page drawn, and the host's word that there are layers. Until this thread takes the view again, the view isn't
/// settled (`Work::telling`), as what's told may queue the session's work.
fn job_ended(shared: &Shared, job: Job, outcome: Outcome, file_changed: bool) {
    let drawn = match (&job.kind, &outcome) {
        (
            JobKind::Draw {
                what: DrawKind::Tile | DrawKind::Region,
                ..
            },
            Outcome::Drawn { .. },
        ) => Some(None),
        (JobKind::Draw { .. }, Outcome::PageFailed { .. }) => Some(Some("page_failed")),
        _ => None,
    };
    let (tab, page) = (job.tab, job.position);
    let mut reader = lock(&shared.reader);
    if file_changed {
        reader.file_changed(tab);
    }
    // What a text job read isn't about what's selected now, if that's gone with the file.
    let outcome = if file_changed && matches!(job.kind, JobKind::Text { .. }) {
        Outcome::Cancelled
    } else {
        outcome
    };
    reader.finish(job, outcome);
    take_pending_follow(shared, &mut reader);
    let has_ops = reader.host.has_ops();
    let note = reader.note().map(str::to_string);
    let selecting = selecting_news(&mut reader);
    // Its end is told with the view let go: until this thread takes the view again, the view isn't settled, as what's told may queue the session's work.
    reader.work.set_telling(true);
    drop(reader);
    shared.before_telling();
    note_change(shared, note);
    tell_selecting(shared, selecting);
    if let Some(failure) = drawn {
        shared.events.page_drawn(tab, page, failure);
    }
    if has_ops {
        signal_ops(shared);
    }
    shared.settled.notify_all();
}

/// **What's been asked of the renderers since the thread last came round** ([`Renew`]): one with the wrong version open is dropped, which kills its worker and
/// closes its file, and the next job opens what's there now; one whose tab is being shown keeps its worker, which says the file's length again to that tab
/// now if it knows it (a view kept whole has nothing left to draw, so no job would come to say it), or at its next job if it doesn't yet. Says what's to be
/// said, for after the page view's lock is let go.
fn renew(
    reader: &mut crate::tiles::Reader,
    renderers: &mut HashMap<PathBuf, OpenDoc>,
    openings: &mut HashMap<PathBuf, Receiver<Result<File, RenderFailure>>>,
) -> Vec<(u32, u32, u64)> {
    let mut told = Vec::new();
    for (file, how) in reader.work.take_renewals() {
        match how {
            Renew::Reopen => {
                renderers.remove(&file);
                openings.remove(&file);
            }
            Renew::TellLength(tab) => {
                let Some(open) = renderers.get_mut(&file) else { continue };
                match (open.doc.known_page_count(), open.doc.known_version()) {
                    (Some(pages), Some(version)) if reader.docs.is_own_file(tab, &file) => {
                        open.length_told = true;
                        told.push((tab, pages, version));
                    }
                    _ => open.length_told = false,
                }
            }
        }
    }
    told
}

/// With nothing to do, this thread is asleep and costs nothing, but for a worker holding a heavy page's memory: see [`big_workers_rest`]. With `retry`, a
/// surface is waited for, and the thread wakes by then at the latest. Idle workers go here when the system asks for memory, whichever it is.
fn rest(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>, policy: &Policy, resting_since: &mut Option<Instant>, retry: Option<Duration>) {
    shared.settled.notify_all();
    if shared.kill_idle_workers.swap(false, Ordering::SeqCst) {
        for open in renderers.values_mut() {
            open.doc.kill_worker();
        }
        publish_workers(shared, renderers);
    }
    match (big_workers_rest(renderers, policy, resting_since), retry) {
        (None, None) => std::thread::park(),
        // Let go just now: the loop says so before it rests again.
        (Some(left), None) if left.is_zero() => {}
        (Some(left), None) => std::thread::park_timeout(left),
        // Waiting for a surface: told here, as the loop would only find the surface refused again, and the wait isn't cut short.
        (Some(left), Some(retry)) if left.is_zero() => {
            publish_workers(shared, renderers);
            std::thread::park_timeout(retry);
        }
        (left, Some(retry)) => std::thread::park_timeout(left.map_or(retry, |left| left.min(retry))),
    }
}

/// **A worker holding a heavy page's memory, which only goes when it does, is let go once nothing's been done for the person for as long as a heavy worker's idle rule** (`Policy::heavy_idle_exit`, 3 s), for the same reason: long enough to bridge a pause in scrolling, and then a heavy page at rest doesn't hold hundreds of megabytes for the 30 s of the idle rule. Looked at when there's nothing to do, and between the jobs for a tab off screen's place, which aren't the person's. Says how long is left, zero when they've just been let go, or `None` when there's none.
fn big_workers_rest(renderers: &mut HashMap<PathBuf, OpenDoc>, policy: &Policy, resting_since: &mut Option<Instant>) -> Option<Duration> {
    let big = |open: &mut OpenDoc| open.doc.worker_footprint().is_some_and(|footprint| footprint > policy.retire_bytes);
    if !renderers.values_mut().any(&big) {
        *resting_since = None;
        return None;
    }
    let left = policy.heavy_idle_exit.saturating_sub(resting_since.get_or_insert_with(Instant::now).elapsed());
    if left.is_zero() {
        for open in renderers.values_mut() {
            if big(open) {
                open.doc.kill_worker();
            }
        }
        *resting_since = None;
    }
    Some(left)
}

/// A page's links, read from the file's structure, here and with no worker.
fn read_links(shared: &Shared, renderers: &HashMap<PathBuf, OpenDoc>, job: Job) {
    // Read from the file's structure, here and with no worker. Only a file a worker already has open and answering, so a disk gone to sleep holds this thread no longer than a draw would: a page that isn't drawn yet has its links read when it is.
    // Through the renderer's own open file and its password, so the links are of the version being drawn, a protected file's too. A file seen replaced and not taken up yet offers none: they'd be the new version's over the old pages.
    let own = lock(&shared.reader).docs.is_own_file(job.tab, &job.file);
    let links = renderers
        .get(&job.file)
        .filter(|open| !open.replaced && !open.doc.file_seen_changed() && open.serves(job.tab, own))
        .and_then(|open| {
            open.doc.read_structure(|file, password| {
                recto_core::nav::page_links_in(file, password, &[job.source_page])
                    .ok()
                    .and_then(|read| read.into_iter().next())
                    .map(|(_, links)| Arc::new(links))
            })
        });
    let mut reader = lock(&shared.reader);
    let outcome = match links {
        Some(found) => Outcome::Links(found),
        None => Outcome::Cancelled,
    };
    reader.finish(job, outcome);
    drop(reader);
    shared.settled.notify_all();
}

fn publish_workers(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>) {
    let mut walled = 0;
    let by_file: Vec<(PathBuf, u32)> = renderers
        .iter_mut()
        .filter_map(|(path, open)| {
            let pid = open.doc.worker_pid()?;
            walled += u32::from(open.doc.worker_walled() == Some(true));
            Some((path.clone(), pid))
        })
        .collect();
    shared.walled_workers.store(walled, Ordering::Relaxed);
    *lock(&shared.worker_pids) = by_file.iter().map(|(_, pid)| *pid).collect();
    *lock(&shared.workers_by_file) = by_file;
}

/// **Only hot and warm tabs' files keep a renderer, and one given a password the person typed only while the tab it was given for holds it as its own
/// file** ([`OpenDoc::keyed_for`]). Dropping one kills its worker and closes its file. A keyed one is a password the person typed, and no job of a tab
/// that only adds its file may use it, so it isn't held for one: otherwise it outlived the tab it was typed for, and a tab opened on the file again, not
/// marked protected as no key was held to read it with, drew on it with nobody asked (the second review of the measured faults). Nor is it held for
/// another tab whose own file is the same: one opened on the file again before this thread came round, with a job of another tab's under way as the
/// first was closed (a whole-suite run of 2026-11-14), would be that tab.
fn let_go_of_unwanted(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>, wanted: &[PathBuf]) {
    renderers.retain(|path, open| wanted.contains(path) && open.keyed_for.is_none_or(|tab| lock(&shared.reader).docs.is_own_file(tab, path)));
}

/// A file's renderer, and what's been decided about its pages on dark paper. Each page gets one decision, however many tiles it comes to, at whatever scale.
struct OpenDoc {
    /// Whoever's asked has been told how long this file is.
    length_told: bool,
    doc: DocRenderer,
    /// Of the file that's open now. A replacement clears them before anything goes by them: see [`seen_replaced`].
    looks: Looks,
    /// The file's been seen to be another one in the middle of a job, and whoever's asked hasn't been told yet.
    replaced: bool,
    /// Its worker was given a password the person typed, and the tab whose job opened it then: for that tab's jobs on its own file, and no other's
    /// ([`OpenDoc::serves`], [`added_needs_password`]). **The key ends with that tab:** a renderer with it is kept only while that tab is held, hot or warm,
    /// with this as its own file ([`let_go_of_unwanted`]), not as long as any tab wants the path, as a document with a page added from the file does, nor
    /// for another tab on the same file. Dropping it kills its worker, whose copy of the password goes with the process, and lets go of its share of the
    /// key, which is overwritten when the keyring lets go too: the keyring keeps a key only while a tab has its file open, cold ones included, so the
    /// password ends when the last tab that owns it closes. `None`: opened with no password, for any tab.
    keyed_for: Option<u32>,
}

impl OpenDoc {
    /// Whether a job of `tab`'s may use this renderer, `own` saying whether the file is the tab's own: any may, with no password given it; with one, only
    /// the tab it was given for, on its own file.
    fn serves(&self, tab: u32, own: bool) -> bool {
        self.keyed_for.is_none_or(|keyed_for| keyed_for == tab && own)
    }
}

/// **The renderer has seen its file replaced: what was decided about the old one's pages goes, now, before anything goes by it.** The renderer opens the new file at the start of whichever job comes next, a draw included, so the tile that job draws is of the new file, and the look it's put on dark paper by has to be too. Says whether it had.
fn seen_replaced(open: &mut OpenDoc) -> bool {
    if !open.doc.take_file_changed() {
        return false;
    }
    open.looks.clear();
    open.replaced = true;
    true
}

/// Why there's no renderer for a job, for now or for good.
enum Unopened {
    Failed(RenderFailure),
    /// The reader left while the file was being opened, so the thread is free for what's wanted now.
    NotWanted,
    /// It hasn't answered in [`OPEN_PATIENCE`], and it's still being tried.
    NotAnswering,
}

/// Opens a file's renderer on a thread of its own, the one part of this that a dead volume can hold up for ever, and waits for it only while the job's still wanted. One file not answering is no reason for a healthy tab to get no tiles. The wait is a look every [`OPEN_LOOK`], and only while a file's being opened.
/// A password the keyring has for the file is given it for `tab`, the tab whose job opens it ([`OpenDoc::keyed_for`]).
fn renderer_for<'a>(
    shared: &Shared,
    renderers: &'a mut HashMap<PathBuf, OpenDoc>,
    openings: &mut HashMap<PathBuf, Receiver<Result<File, RenderFailure>>>,
    tab: u32,
    path: &Path,
    service: &Result<ServiceConfig, RenderFailure>,
    policy: &Policy,
) -> Result<&'a mut OpenDoc, Unopened> {
    if !renderers.contains_key(path) {
        let config = service.as_ref().map_err(|failure| Unopened::Failed(*failure))?.clone();
        if !openings.contains_key(path) {
            if openings.len() >= MAX_OPENINGS {
                return Err(Unopened::NotAnswering);
            }
            let (sender, receiver) = sync_channel(1);
            let opening = path.to_path_buf();
            std::thread::Builder::new()
                .name("recto-open".into())
                .stack_size(128 * 1024)
                .spawn(move || {
                    let _ = sender.send(DocRenderer::open_file(&opening));
                })
                .map_err(|_| Unopened::Failed(RenderFailure::Unavailable))?;
            openings.insert(path.to_path_buf(), receiver);
        }
        let started = Instant::now();
        let opened = loop {
            let receiver = openings.get(path).ok_or(Unopened::Failed(RenderFailure::Unavailable))?;
            match receiver.recv_timeout(OPEN_LOOK) {
                Ok(opened) => break opened,
                Err(RecvTimeoutError::Disconnected) => break Err(RenderFailure::Unavailable),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if !lock(&shared.reader).running_is_wanted() {
                return Err(Unopened::NotWanted);
            }
            if started.elapsed() >= OPEN_PATIENCE {
                return Err(Unopened::NotAnswering);
            }
        };
        openings.remove(path);
        let file = opened.map_err(Unopened::Failed)?;
        // By the file that was opened, not whatever's at its path now.
        let key = shared
            .config
            .keys
            .as_ref()
            .filter(|keys| !keys.is_empty())
            .and_then(|keys| keys.get(recto_platform::secret::open_file_identity(&file)?));
        let keyed_for = key.is_some().then_some(tab);
        let doc = DocRenderer::with_file(file, path, key, config, policy.clone()).map_err(Unopened::Failed)?;
        renderers.insert(
            path.to_path_buf(),
            OpenDoc {
                length_told: false,
                doc,
                looks: Looks::default(),
                replaced: false,
                keyed_for,
            },
        );
    }
    renderers.get_mut(path).ok_or(Unopened::Failed(RenderFailure::Unavailable))
}

/// **A password is for the tab it was typed for** (review round 1 of the measured faults, S1). A page added to another document from a file that needs
/// one, which the loader that adds pages didn't (a crafted file the two readers see differently, as nothing else needing a password can be added), isn't
/// drawn, read or linked from with the password the person typed for that file's own tab, and its document isn't asked for one: the page says why and
/// draws nothing, as any page that can't be drawn. So nothing a password protects is shown in, pictured in the sleep file of, or read by an agent from a
/// document it wasn't typed for. Workers are shared by path, so it's the job's tab that decides, not who opened the file first. An added file that needs
/// no password is drawn as ever.
fn added_needs_password(job: &Job) -> Outcome {
    match job.kind {
        JobKind::Text { .. } => Outcome::TextFailed,
        JobKind::Links => Outcome::Cancelled,
        _ => Outcome::PageFailed {
            message: ADDED_FILE_NEEDS_PASSWORD.to_string(),
            may_pass: false,
        },
    }
}

/// Whether the job's file is an added PDF that's changed since its pages were added, and whether it's the tab's own file. **An added PDF that's changed
/// isn't drawn from** (the saved point's review): a stat, before each job, since drawing goes by path.
fn added_file_now(shared: &Shared, job: &Job) -> (bool, bool) {
    let (expected, own) = {
        let reader = lock(&shared.reader);
        (reader.docs.expected_stamp(job.tab, &job.file), reader.docs.is_own_file(job.tab, &job.file))
    };
    let changed =
        expected.is_some_and(|expected| !recto_platform::file_stamp(&job.file).is_ok_and(|now| now.same_contents(expected) && now.inode == expected.inode));
    (changed, own)
}

/// What a job for a PDF added to the tab came to: a want of a password is the page's, never its document's ([`added_needs_password`]).
fn as_an_added_files(outcome: Outcome, job: &Job) -> Outcome {
    match outcome {
        Outcome::DocumentFailed { needs_key: true, .. } => added_needs_password(job),
        outcome => outcome,
    }
}

/// A failure that's about the whole file, saying when it's for want of a password, which the app can ask for again.
fn document_failed(failure: RenderFailure) -> Outcome {
    Outcome::DocumentFailed {
        message: failure.message(),
        needs_key: matches!(failure, RenderFailure::NeedsPassword),
    }
}

fn do_job(open: &mut OpenDoc, job: &Job) -> Outcome {
    if let JobKind::Text { check } = job.kind {
        let read = open.doc.page_characters_for(job.source_page, true, check, job.generation);
        // Told to stop, a check that didn't finish says nothing about the page, so it's as though it was never tried. What was read on the way is good, and a read that went with its worker isn't a failure of the page's.
        let stopped = open.doc.is_stale(job.generation);
        return match read {
            Ok(page) => Outcome::Text {
                text: Arc::new(PageText::build(&page.chars, &page.boxes, page.frame, page.hidden_checked, page.copy_restricted)),
                checked: check && !stopped,
            },
            Err(_) if stopped => Outcome::Cancelled,
            Err(_) => Outcome::TextFailed,
        };
    }
    let doc = &mut open.doc;
    match &job.kind {
        JobKind::Text { .. } => Outcome::TextFailed,
        // Read before a renderer's asked for: see the thread's loop.
        JobKind::Links => Outcome::Links(None),
        JobKind::Size => match doc.page_size(job.source_page) {
            Ok((width, height)) => Outcome::Size(PageBox { width, height }),
            Err(RenderFailure::WouldStart { heavy }) => Outcome::Unstarted { heavy },
            Err(failure) if whole_file(failure) => document_failed(failure),
            Err(failure) => Outcome::PageFailed {
                message: failure.message(),
                may_pass: false,
            },
        },
        JobKind::Traits => match doc.largest_image_pixels(job.source_page, job.generation) {
            // A stale question answers zero, which isn't an answer about the page. Remembering it would draw a heavy page a tile at a time.
            Ok(_) if doc.is_stale(job.generation) => Outcome::Cancelled,
            Ok(pixels) => Outcome::Heavy(pixels >= recto_render_service::REGION_MIN_IMAGE_PIXELS),
            Err(RenderFailure::WouldStart { heavy }) => Outcome::Unstarted { heavy },
            Err(failure) if whole_file(failure) => document_failed(failure),
            Err(_) => Outcome::Heavy(false),
        },
        JobKind::Draw {
            scale,
            rect,
            surface,
            what,
            paper,
        } => {
            let request = TileRequest {
                page: job.source_page,
                rect: *rect,
                scale: *scale,
                stride: surface.stride(),
                generation: job.generation,
            };
            let result = match what {
                DrawKind::Region => doc.render_region(&request, recto_proto::TILE_EDGE, surface),
                DrawKind::Tile | DrawKind::Preview => doc.render_tile(&request, surface),
            };
            match result {
                Ok(TileResult::Done(tile)) => {
                    let mut kind = tile.kind;
                    let low_res = tile.low_res.map(|low| (low.rect, low.why));
                    let low_res_scale = tile.low_res.map(|low| low.scale);
                    if *paper == PaperKind::Dark {
                        let drawn = low_res.map_or(*rect, |(drawn, _)| drawn);
                        let drawn_scale = low_res_scale.unwrap_or(*scale);
                        // A preview is a picture of the whole page, as drawn for white paper, which is what's wanted to judge it: no extra draw.
                        let whole = (*what == DrawKind::Preview && low_res.is_none()).then_some(*rect);
                        let tile = (whole.is_none() && low_res.is_none()).then_some((surface, *rect, *scale));
                        // Replaced as this was drawn, so it's of the new file, and no look of the old one's may go for it.
                        seen_replaced(open);
                        // What's drawn for a tab off screen with its look or size refused for want of a worker goes nowhere: a picture put on dark paper by a look decided without them isn't the page's.
                        // A look gone stale or to be tried again goes nowhere either: the tile isn't handed on, and what's shown of the page stays.
                        let look = tile_look(
                            &mut open.doc,
                            &mut open.looks,
                            job.source_page,
                            job.generation,
                            whole.map(|rect| (surface, rect)),
                            tile,
                            Instant::now(),
                        );
                        if let Err(Outcome::Unstarted { heavy }) = look {
                            return Outcome::Unstarted { heavy };
                        }
                        let size = match look.as_ref().ok().map(|_| open.doc.page_size(job.source_page)) {
                            Some(Err(RenderFailure::WouldStart { heavy })) => return Outcome::Unstarted { heavy },
                            size => size.and_then(Result::ok),
                        };
                        let page = size.and_then(|(width, height)| recto_proto::device_size(width, height, drawn_scale).ok());
                        // Replaced after it was drawn, while its look or size was being asked: the tile's of the old file, and the look may be of either, or of a draw that failed for it. Neither is kept, and the tile's drawn again.
                        if seen_replaced(open) {
                            return Outcome::Cancelled;
                        }
                        let look = match look {
                            Ok(look) => look,
                            Err(outcome) => return outcome,
                        };
                        // What keeps no surface comes down to its one colour. With the lower-resolution version, the surface is kept whatever's in it.
                        let colour_only = *what == DrawKind::Tile && low_res.is_none();
                        let dark = look.in_tile(page, drawn);
                        let _ = surface.with_pixels(|pixels| kind = onto_dark_paper(pixels, drawn, surface.stride(), kind, dark, colour_only));
                    }
                    Outcome::Drawn { kind, low_res }
                }
                Ok(TileResult::Cancelled { .. }) => Outcome::Cancelled,
                Err(RenderFailure::WouldStart { heavy }) => Outcome::Unstarted { heavy },
                Err(failure) if whole_file(failure) => document_failed(failure),
                Err(failure @ RenderFailure::NotDrawableNow { .. }) => Outcome::PageFailed {
                    message: failure.message(),
                    may_pass: true,
                },
                Err(failure) => Outcome::PageFailed {
                    message: failure.message(),
                    may_pass: false,
                },
            }
        }
    }
}

/// Puts one tile on dark paper, as the rule says, and says what it is now. With `colour_only` a tile of one colour needs nothing done to its pixels, as it won't keep them.
pub(super) fn onto_dark_paper(pixels: &mut [u8], drawn: recto_proto::TileRect, stride: u32, kind: TileKind, dark: DarkTile, colour_only: bool) -> TileKind {
    match (dark, kind) {
        (DarkTile::Dim, TileKind::Blank { rgb }) if colour_only => TileKind::Blank { rgb: dim_rgb(rgb) },
        (DarkTile::TurnOver(keeps, whiten), TileKind::Blank { rgb }) if keeps.is_empty() && whiten.is_empty() && colour_only => {
            TileKind::Blank { rgb: dark_rgb(rgb) }
        }
        (DarkTile::Dim, _) => {
            dim_page(pixels, drawn.width, drawn.height, stride);
            if matches!(kind, TileKind::Blank { .. }) { TileKind::Colour } else { kind }
        }
        (DarkTile::TurnOver(keeps, whiten), _) => {
            turn_over(pixels, drawn.width, drawn.height, stride, is_grey(kind), &keeps, &whiten);
            // Part of it stays a picture, so it isn't one colour now.
            if matches!(kind, TileKind::Blank { .. }) { TileKind::Colour } else { kind }
        }
    }
}

/// Failures about the file or the renderer rather than one page.
fn whole_file(failure: RenderFailure) -> bool {
    !matches!(
        failure,
        RenderFailure::BadPage
            | RenderFailure::BadRequest
            | RenderFailure::TooLargeAtThisScale
            | RenderFailure::PageFailed { .. }
            | RenderFailure::PageReadFailed { .. }
            | RenderFailure::NotDrawableNow { .. }
            | RenderFailure::ExportRefused { .. }
    )
}
