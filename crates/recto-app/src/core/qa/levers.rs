//! **The QA levers that need no window:** a test's conditions on the session (sleep, budgets, memory pressure, held reads and drawing), what a record
//! brings back that no action makes, and the save whose writing a test does. Each is a verb in the table ([`recto_agent::commands::qa::verbs`]), answered
//! here and never by the host.

use crate::core::{Inner, save_from_signature_check, saving};
use crate::queue::CoreCall;
use recto_agent::AgentError;
use recto_agent::Code;
use std::path::PathBuf;
use std::time::Duration;

impl Inner {
    /// A QA verb that needs no window. `None` if it's for the host. Calls what the UI will.
    pub fn qa_session_verb(&self, verb: &str, value: &str) -> Option<Result<(), AgentError>> {
        let usage = |words: String| AgentError::new(Code::USAGE, words, recto_agent::exit::USAGE);
        let session = &self.host.session;
        let result = match verb {
            // What a record brings back that no one action makes: text in Find's field that wasn't found for.
            "find_draft" => session.with_session(|session| {
                let find = session.viewing().find().clone();
                session.viewing_mut().set_find(find.open, &find.query, value);
                Ok(())
            }),
            "view_rotation" => match value.parse::<u8>() {
                Ok(turns) if turns < 4 => session.with_session(|session| {
                    if let Some(id) = session.tabs().active() {
                        session.tabs_mut().set_view_rotation(id, turns);
                    }
                    Ok(())
                }),
                _ => Err(usage("It's a number of quarter turns, from 0 to 3.".to_string())),
            },
            "save_from" => self.qa_save_from(value),
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
                self.life.hold_reads(value == "on");
                Ok(())
            }
            _ => return None,
        };
        self.core.push(CoreCall::Reconcile);
        Some(result)
    }

    /// **What a Save does around its writing, with the writing done by the test,** for crash and restore tests: keeps what's needed, notes what's coming and flushes that, puts the file in place, and confirms. Every step but the writing is the real one, and it's refused where Save would be.
    pub fn qa_save_from(&self, written: &str) -> Result<(), AgentError> {
        let failed = |what: &str| AgentError::new(Code::SAVE_FAILED, format!("{what} failed."), recto_agent::exit::FAILED);
        let written = PathBuf::from(written);
        // The second line behind the table's (its value is a file to the table): a file outside the run's folders is refused before anything is looked
        // at, so its being there or not says nothing, and it's never moved into a tab (2026-11-13).
        if let Some(folders) = self.host.session.confinement() {
            recto_agent::quiet_files::check_paths(&folders, std::slice::from_ref(&written))?;
        }
        let target = recto_platform::file_stamp(&written).map_err(|_| failed("Looking at the written file"))?;
        // The checks Save makes before it writes: never over a signed file, never over one changed on disk, never while it's being saved.
        let (path, known, saving) = self
            .host
            .session
            .with_session(|session| {
                session
                    .tabs()
                    .active()
                    .and_then(|id| Some((session.tabs().path_of(id)?, session.tabs().own_stamp(id), session.saves().is_saving(id))))
            })
            .ok_or_else(|| AgentError::new(Code::NO_ACTIVE_TAB, "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        if saving {
            return Err(AgentError::new(Code::SAVING, "That document is being saved.", recto_agent::exit::FAILED));
        }
        if known.is_none_or(|known| recto_platform::file_stamp(&path).ok() != Some(known)) {
            return Err(AgentError::new(
                Code::FILE_CHANGED,
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
                let id = session.tabs().active()?;
                let needed = session.lists().own_pages_needed(id);
                Some((id, session.tabs().path_of(id)?, session.pages().count(id), needed))
            })
            .ok_or_else(|| AgentError::new(Code::NO_ACTIVE_TAB, "No tab is open.", recto_agent::exit::NOT_FOUND))?;
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
