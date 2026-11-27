//! Where a page's ink comes from for the check for hidden text: a render worker, started here on the first page that has headings and ended with the job. The structure helper can't have it
//! (walled off as the page drawer is, it has no way to start one), so what it found is checked here, by [`recto_index::generated::visible::keep_visible`].
//!
//! The worker's limit is the indexer's: only this check draws, and a page it can't answer for gets one more go with a fresh worker, since a heading that can't be checked isn't kept. It
//! runs at the renderer's own level (PLAN 8.3: S6b-2 measures whether that is the right one for a job somebody is waiting on).

use recto_index::generated::visible::{InkSource, PageInk};
use recto_render_service::{DocRenderer, Policy, RenderStats, ServiceConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The render worker's limit, as the indexer's: only the check for hidden text draws.
const WORKER_MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
const WORKER_PAGE_DEADLINE: Duration = Duration::from_secs(10);

/// The render worker for one job. It stops being asked at the job's `deadline` or when `stop` is set, and says which in [`Ink::stopped_for`].
pub(crate) struct Ink<'a> {
    pub path: PathBuf,
    renderer: Option<DocRenderer>,
    /// It couldn't be started: no page can be checked, so none of their headings are kept.
    pub unavailable: bool,
    pub deadline: Instant,
    pub stop: &'a AtomicBool,
    /// Why no more pages are asked for: `out_of_time` or `cancelled`.
    pub stopped_for: Option<&'static str>,
}

impl<'a> Ink<'a> {
    pub fn new(path: PathBuf, deadline: Instant, stop: &'a AtomicBool) -> Self {
        Self {
            path,
            renderer: None,
            unavailable: false,
            deadline,
            stop,
            stopped_for: None,
        }
    }

    pub fn stats(&self) -> RenderStats {
        self.renderer.as_ref().map(DocRenderer::stats).unwrap_or_default()
    }

    /// Whether the job is over: past its time or no longer wanted. Once it is, it stays so.
    pub(crate) fn over(&mut self) -> bool {
        if self.stopped_for.is_none() {
            self.stopped_for = if Instant::now() > self.deadline {
                Some("out_of_time")
            } else if self.stop.load(Ordering::SeqCst) {
                Some("cancelled")
            } else {
                None
            };
        }
        self.stopped_for.is_some()
    }

    fn page_once(&mut self, page: u32) -> Option<PageInk> {
        if self.unavailable {
            return None;
        }
        if self.renderer.is_none() {
            let policy = Policy {
                memory_limit_bytes: WORKER_MEMORY_LIMIT,
                heavy_limit_bytes: WORKER_MEMORY_LIMIT,
                page_deadline: WORKER_PAGE_DEADLINE,
                ..Policy::default()
            };
            match ServiceConfig::discover().and_then(|config| DocRenderer::new(&self.path, None, config, policy)) {
                Ok(renderer) => self.renderer = Some(renderer),
                Err(_) => {
                    self.unavailable = true;
                    return None;
                }
            }
        }
        let characters = self.renderer.as_mut()?.page_characters(page, true, true).ok()?;
        Some(PageInk {
            chars: characters.chars,
            boxes: characters.boxes,
            frame: characters.frame,
            checked: characters.hidden_checked,
        })
    }
}

impl InkSource for Ink<'_> {
    fn over(&mut self) -> bool {
        Ink::over(self)
    }

    /// A page the worker couldn't answer for, past its deadline on a starved machine or with the worker gone, gets one more go with a fresh worker: otherwise every heading on it is dropped unseen, as one that can't be checked isn't kept (CI, 2026-11-11: five of six headings, on a loaded virtual machine).
    fn page(&mut self, page: u32) -> Option<PageInk> {
        if self.over() {
            return None;
        }
        if let Some(read) = self.page_once(page).filter(|read| read.checked) {
            return Some(read);
        }
        if self.over() {
            return None;
        }
        self.renderer = None;
        self.page_once(page)
    }
}
