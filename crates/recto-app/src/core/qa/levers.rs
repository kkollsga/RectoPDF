//! **The QA levers that need no window:** a test's conditions on the session (sleep, budgets, memory pressure, held reads and drawing), what a record
//! brings back that no action makes, and the save whose writing a test does. Each is a verb in the table ([`recto_agent::commands::qa::verbs`]), answered
//! here and never by the host.

use crate::core::{Inner, save_from_signature_check, saving};
use crate::lock;
use crate::queue::CoreCall;
use recto_agent::AgentError;
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl Inner {
    /// A QA verb that needs no window. `None` if it's for the host. Calls what the UI will.
    pub fn qa_session_verb(&self, verb: &str, value: &str) -> Option<Result<(), AgentError>> {
        let usage = |words: String| AgentError::new("usage", words, recto_agent::exit::USAGE);
        let session = &self.host.session;
        let result = match verb {
            // What ⌘F and Escape do.
            "find" => {
                let (open, close) = (value == "open", value != "open");
                let find = recto_agent::commands::finding::Find {
                    open,
                    close,
                    ..Default::default()
                };
                self.core.push(CoreCall::Run(crate::queue::PersonCommand::input(find.into())));
                self.core.flush(Duration::from_secs(5));
                Ok(())
            }
            "find_query" => session.with_session(|session| {
                let find = session.find().clone();
                session.set_find(find.open, value, &find.draft);
                Ok(())
            }),
            "find_draft" => session.with_session(|session| {
                let find = session.find().clone();
                session.set_find(find.open, &find.query, value);
                Ok(())
            }),
            // What going in does, and what Done does: the person's `edit mode on|off`.
            "edit_mode" => {
                if let Some(inner) = self.saver.inner() {
                    inner.edit_mode_for_person(value == "on");
                }
                self.core.flush(Duration::from_secs(5));
                Ok(())
            }
            "view_rotation" => match value.parse::<u8>() {
                Ok(turns) if turns < 4 => session.with_session(|session| {
                    if let Some(id) = session.active() {
                        session.set_view_rotation(id, turns);
                    }
                    Ok(())
                }),
                _ => Err(usage("It's a number of quarter turns, from 0 to 3.".to_string())),
            },
            "save_from" => self.qa_save_from(value),
            // What Save As's sheet does when it's answered with this path: the same call.
            "save_as" => match self.saver.inner() {
                Some(inner) if value.starts_with('/') => match lock(&inner.published).reader.shown_tab {
                    0 => Err(AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND)),
                    tab => {
                        saving::start_save(&inner, TabId(tab), Some(PathBuf::from(value)), false);
                        Ok(())
                    }
                },
                _ => Err(usage("It's the whole path of a new file.".to_string())),
            },
            "dormant_after" => match value.parse::<u32>() {
                Ok(seconds) => {
                    self.sleeper.set_after(seconds);
                    self.core.push(CoreCall::Reconcile);
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of seconds.".to_string())),
            },
            "live_budget" => match value.parse::<u64>() {
                Ok(mib) => {
                    self.sleeper.set_budget(mib.saturating_mul(1024 * 1024));
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of MiB.".to_string())),
            },
            "hold_drawing" => {
                for engine in self.panes.engines(&self.engine) {
                    engine.hold_drawing(value == "on");
                }
                Ok(())
            }
            "sleep_write_gap" => match value.parse::<u32>() {
                Ok(seconds) => {
                    self.sleeper.set_write_gap(seconds);
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of seconds.".to_string())),
            },
            "memory_pressure" => match value {
                "warning" | "critical" => {
                    self.core.push(CoreCall::MemoryPressure(value == "critical"));
                    Ok(())
                }
                // What the host's dispatch source calls, through the same method, which in a quiet run only counts it.
                "system-warning" | "system-critical" => {
                    self.system_pressure(value == "system-critical");
                    Ok(())
                }
                // The system taking back what's marked discardable, which it does without a word whenever it needs the pages.
                "discard" => {
                    for engine in self.panes.engines(&self.engine) {
                        engine.empty_discardable();
                    }
                    Ok(())
                }
                _ => Err(usage("It's `warning`, `critical` or `discard`.".to_string())),
            },
            "note_input" => {
                self.note_input();
                Ok(())
            }
            "hold_reads" => {
                self.life.qa_hold_reads.store(value == "on", Ordering::SeqCst);
                Ok(())
            }
            _ => return None,
        };
        self.core.push(CoreCall::Reconcile);
        Some(result)
    }

    /// **What a Save does around its writing, with the writing done by the test,** for crash and restore tests: keeps what's needed, notes what's coming and flushes that, puts the file in place, and confirms. Every step but the writing is the real one, and it's refused where Save would be.
    pub fn qa_save_from(&self, written: &str) -> Result<(), AgentError> {
        let failed = |what: &str| AgentError::new("save_failed", format!("{what} failed."), recto_agent::exit::FAILED);
        let written = PathBuf::from(written);
        let target = recto_platform::file_stamp(&written).map_err(|_| failed("Looking at the written file"))?;
        // The checks Save makes before it writes: never over a signed file, never over one changed on disk, never while it's being saved.
        let (path, known, saving) = self
            .host
            .session
            .with_session(|session| {
                session
                    .active()
                    .and_then(|id| Some((session.path_of(id)?, session.own_stamp(id), session.is_saving(id))))
            })
            .ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        if saving {
            return Err(AgentError::new("saving", "That document is being saved.", recto_agent::exit::FAILED));
        }
        if known.is_none_or(|known| recto_platform::file_stamp(&path).ok() != Some(known)) {
            return Err(AgentError::new(
                "file_changed",
                recto_core::save::SaveError::FileChanged.message(),
                recto_agent::exit::FAILED,
            ));
        }
        save_from_signature_check(recto_core::save::signature_facts_of(&path))?;
        saving::save_from_gate(&path, known)?;
        let (id, path, pages, needed) = self
            .host
            .session
            .with_session(|session| {
                let id = session.active()?;
                let needed = session.page_edits(id).map(recto_session::edit::PageEdits::own_pages_needed).unwrap_or_default();
                Some((id, session.path_of(id)?, session.pages_in(id), needed))
            })
            .ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        let kept = self.saver.store.keep_pages_of(&path, target, &needed);
        self.host.session.with_session(|session| session.save_prepared(id, target, kept.as_ref()));
        self.saver
            .flush(false, Duration::from_secs(5))
            .map_err(|_| failed("Writing what's about to happen"))?;
        std::fs::rename(&written, &path).map_err(|_| failed("Putting the file in place"))?;
        let now = recto_platform::file_stamp(&path).map_err(|_| failed("Looking at the saved file"))?;
        self.host.session.with_session(|session| session.saved(id, now, pages, None, kept));
        Ok(())
    }
}
