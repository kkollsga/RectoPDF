//! **`recto find`, for whoever asks** (step R2.3, C5g): the app's Find, by the one handler, the person's at the window and an agent's alike. What tells them
//! apart is said once each, by [`Actor`]:
//!
//! - **An agent's `find` waits; the person's returns at once.** An agent's query answers with its hits, and its step with the hit it went to, each waiting up
//!   to [`COMMAND_WAIT`]. The window draws what the session publishes as it comes, so the person's query returns as it starts, and a step asked for before
//!   its hit is known is kept for when it is ([`Effect::FindStepPending`]).
//! - **The person's field is theirs.** What they type stays in the field as the draft whether it can be found or not, and it's found with their passwords, so
//!   a document they opened with one is searched for them. An agent's `find` never gets those keys ([`crate::SessionAccess::find_keys`]).
//! - **A6: a person's Find shows the sidebar it's in; an agent's `find` doesn't rearrange the person's window.** Find opens for both, and only the person's
//!   brings the sidebar back.

use super::*;
use crate::{Actor, Effect, Event};
use serde_json::{Value, json};

/// How long `recto find` waits for a query to finish before answering with what's found so far.
const COMMAND_WAIT: Duration = Duration::from_secs(30);
/// Hits in an answer, if not told otherwise, and at most.
const DEFAULT_LISTED: u32 = 20;
const MAX_LISTED: u32 = 200;

/// What `recto find` was asked.
pub(crate) struct Asked<'a> {
    pub query: Option<&'a str>,
    pub next: bool,
    pub previous: bool,
    /// The step goes to the hit as a jump, marked for Back, as Return in the field does.
    pub jump: bool,
    /// A hit by its index in the list, as a click on its row.
    pub hit: Option<u32>,
    pub open: bool,
    pub close: bool,
    pub limit: Option<u32>,
}

/// Results kept for a session that can't find in the background, such as `recto local`: one command's worth.
struct Here(std::sync::Mutex<Results>);

impl Finding for Here {
    fn start(&self, _plan: FindPlan, _query: &str, _needle: String) -> u64 {
        0
    }
    fn clear(&self) {}
    fn with_results(&self, step: &mut dyn FnMut(&mut Results)) {
        step(&mut self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    }
    fn wait(&self, _generation: u64, _limit: Duration) -> bool {
        true
    }
    // A run here is over before anything looks at it.
    fn wait_past(&self, _version: u64, _limit: Duration) -> bool {
        true
    }
}

/// One command's reach: the session, who asked, and the one wait for the whole command, a query's and a step's together, inside the client's patience.
struct Run<'a, A> {
    access: &'a A,
    actor: Actor,
    finding: &'a dyn Finding,
    here: &'a Here,
    deadline: Instant,
}

/// `recto find`: with a query, finds it in the document being read; `--next` and `--previous` go to a hit from where the reader is, `--jump` as a jump;
/// `--hit` goes to one by its index; `--open` opens Find with what was typed last found again; `--close` closes it. Always says where Find stands.
pub(crate) fn command<A: crate::SessionAccess>(access: &A, actor: Actor, asked: &Asked<'_>, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let listed = crate::positive_limit(asked.limit, DEFAULT_LISTED, MAX_LISTED)?;
    one_thing(asked)?;
    let here = Here(std::sync::Mutex::new(Results::default()));
    let run = Run {
        access,
        actor,
        finding: access.finding().unwrap_or(&here),
        here: &here,
        deadline: Instant::now() + COMMAND_WAIT,
    };
    let went = if asked.close {
        // Announced when Find changed, and only then: closing it closed already changes nothing.
        let before = access.with(|s| s.viewing().find().clone());
        close(access);
        if access.with(|s| *s.viewing().find() != before) {
            events.push(Event::FindClosed);
        }
        None
    } else if asked.open {
        run.open(events)?;
        None
    } else if let Some(query) = asked.query {
        run.query(query, events)?;
        None
    } else if asked.next || asked.previous {
        let going = if asked.jump { Going::Jump(actor.author()) } else { Going::Step };
        run.step(asked.next, going, events)?
    } else if let Some(index) = asked.hit {
        run.hit(index as usize, events)?
    } else {
        None
    };
    // The window draws what the session publishes, and reads no answer.
    if !access.answer_read() {
        return Ok(Value::Null);
    }
    Ok(answer(access, run.finding, listed, went))
}

/// One thing at a time: a query, a step, a hit, opening or closing.
fn one_thing(asked: &Asked<'_>) -> Result<(), AgentError> {
    let said = [
        asked.query.is_some(),
        asked.next || asked.previous,
        asked.hit.is_some(),
        asked.open,
        asked.close,
    ];
    if said.iter().filter(|said| **said).count() > 1 || (asked.next && asked.previous) {
        return Err(AgentError::new(
            "usage",
            "That's more than one thing at once: a query, a step, a hit, opening or closing.",
            exit::USAGE,
        )
        .then("Find first, then `recto find --next`."));
    }
    if asked.jump && !(asked.next || asked.previous) {
        return Err(AgentError::new("usage", "`--jump` goes with `--next` or `--previous`.", exit::USAGE).then("`recto find --next --jump`."));
    }
    Ok(())
}

/// Whether a query's running.
fn running(finding: &dyn Finding) -> bool {
    let mut running = false;
    finding.with_results(&mut |results| running = results.running());
    running
}

/// The refusal when there's no hit to go to.
fn nothing_found() -> AgentError {
    AgentError::new("nothing_found", "There's no hit to go to.", exit::NOT_FOUND).then("Find something first: `recto find \"words\"`.")
}

impl<A: crate::SessionAccess> Run<'_, A> {
    /// Whether this is the person's, from the window: it returns at once, and its Find shows the sidebar (A6).
    fn at_the_window(&self) -> bool {
        self.actor.at_the_window()
    }

    /// Starts finding `query`: the person's with their passwords, and an agent's with none, waited for. The person's field keeps what they typed as its
    /// draft, cut to what the record keeps, even when it can't be found, as when it's too long, which is the refusal.
    fn query(&self, query: &str, events: &mut Vec<Event>) -> Result<(), AgentError> {
        let access = self.access;
        if self.at_the_window() {
            let mut end = query.len().min(recto_session::record::MAX_FIND_BYTES);
            while !query.is_char_boundary(end) {
                end -= 1;
            }
            let typed = &query[..end];
            events.push(Event::FindOpened { page: None });
            return begin_keyed(access, typed, access.find_keys(self.actor)).map(drop).inspect_err(|_| {
                self.finding.clear();
                access.with(|s| s.viewing_mut().set_find(true, "", typed));
            });
        }
        let generation = begin(access, query)?;
        if access.finding().is_some() {
            self.finding.wait(generation, self.deadline.saturating_duration_since(Instant::now()));
        } else {
            self.here_to_the_end(query)?;
        }
        events.push(Event::FindOpened { page: None });
        Ok(())
    }

    /// Finds `query` here, to the end, for a session that can't find in the background.
    fn here_to_the_end(&self, query: &str) -> Result<(), AgentError> {
        let (plan, needle) = self.access.with(|s| {
            let id = s
                .active()
                .ok_or_else(|| AgentError::new("no_active_tab", "There's no document being read to find in.", exit::NOT_FOUND))?;
            Ok::<_, AgentError>((plan(s, id)?, needle(query).unwrap_or_default()))
        })?;
        let mut results = Results::default();
        let generation = results.begin_on(&plan, query);
        let stop = AtomicBool::new(false);
        let (end, progress) = run(&plan, &needle, self.access.indexing(), &stop, &mut |hits, progress| {
            results.add(generation, hits, progress)
        });
        results.finish(generation, end, progress);
        self.here.with_results(&mut |kept| *kept = std::mem::take(&mut results));
        Ok(())
    }

    /// ⌘F: Find opened, with what was typed last found again, as it would be on a Mac: it's been closed, so its hits went. The person's shows the sidebar it's
    /// in (A6). Announced when Find changed, and only then: ⌘F with it open already only gives its field the keyboard.
    fn open(&self, events: &mut Vec<Event>) -> Result<(), AgentError> {
        let access = self.access;
        let before = access.with(|s| s.viewing().find().clone());
        if self.at_the_window() {
            access.with(|s| {
                let mut view = s.viewing().view();
                if view.sidebar_hidden {
                    view.sidebar_hidden = false;
                    s.viewing_mut().set_view(view);
                    events.push(crate::view_changed(view));
                }
            });
        }
        let (query, draft) = access.with(|s| {
            let find = s.viewing().find().clone();
            s.viewing_mut().set_find(true, &find.query, &find.draft);
            (find.query, find.draft)
        });
        let mut found = Ok(());
        if query.is_empty() && !draft.is_empty() {
            let mut quiet = Vec::new();
            found = self.query(&draft, &mut quiet);
        }
        if access.with(|s| *s.viewing().find() != before) {
            events.push(Event::FindOpened { page: None });
        }
        // An agent's Find is open, and its answer says what the draft found; the person's field says why it found nothing.
        if self.at_the_window() { found } else { Ok(()) }
    }

    /// A step needs the hits of what's in the field, for the document being read: found again if they're of something else, as after a restart. The
    /// person's ⌘G with Find closed opens it, with what was typed last, as on a Mac. Says whether a query's been started for it.
    fn hits_for_a_step(&self, events: &mut Vec<Event>) -> Result<bool, AgentError> {
        if self.at_the_window() && self.access.with(|s| !s.viewing().find().open || s.viewing().find().query.is_empty()) {
            self.open(events)?;
        }
        let Some(query) = stale_query(self.access, self.finding) else {
            return Ok(false);
        };
        if self.at_the_window() {
            begin_keyed(self.access, &query, self.access.find_keys(self.actor))?;
        } else {
            let generation = begin(self.access, &query)?;
            self.finding.wait(generation, self.deadline.saturating_duration_since(Instant::now()));
        }
        Ok(true)
    }

    /// `--next` or `--previous`: an agent's waits for its hit; the person's goes now, or is kept for when its hit is known, as a query still running may not
    /// have reached it.
    fn step(&self, forward: bool, going: Going, events: &mut Vec<Event>) -> Result<Option<(u32, Hit)>, AgentError> {
        let (access, finding) = (self.access, self.finding);
        let restarted = self.hits_for_a_step(events)?;
        if matches!(going, Going::Jump(_)) {
            access.take_places();
        }
        let went = if self.at_the_window() {
            let went = (!restarted).then(|| step(access, finding, forward, going)).flatten();
            if went.is_none() && running(finding) {
                access.effect(Effect::FindStepPending { forward, going });
                return Ok(None);
            }
            went
        } else {
            step_waiting(access, finding, forward, going, self.deadline.saturating_duration_since(Instant::now()))
        };
        Ok(Some(self.went(went.ok_or_else(nothing_found)?, events)))
    }

    /// `--hit N`: the hit with that index in the list, as a click on its row does, a jump. An agent's waits for it while the query runs.
    fn hit(&self, index: usize, events: &mut Vec<Event>) -> Result<Option<(u32, Hit)>, AgentError> {
        let (access, finding, by) = (self.access, self.finding, self.actor.author());
        access.take_places();
        let went = if self.at_the_window() {
            choose(access, finding, index, by)
        } else {
            self.hits_for_a_step(events)?;
            waiting_for(finding, self.deadline.saturating_duration_since(Instant::now()), || {
                choose(access, finding, index, by)
            })
        };
        let went = went.ok_or_else(|| {
            AgentError::new(
                "no_such_hit",
                "There's no hit with that index to go to: it's past the end, or on text that looks hidden.",
                exit::NOT_FOUND,
            )
            .then("`recto find` lists the hits with their `index`, counted from 0.")
        })?;
        Ok(Some(self.went(went, events)))
    }

    /// A hit gone to: heard, and selected on the page, as ⌘G's is, whoever asked.
    fn went(&self, (tab, hit): (u32, Hit), events: &mut Vec<Event>) -> (u32, Hit) {
        self.access.with(|s| heard_going_to(s, tab, &hit, events));
        self.access.effect(Effect::Reveal {
            tab: TabId(tab),
            hit: hit.clone(),
        });
        (tab, hit)
    }
}

/// Where Find stands, for an answer.
fn answer<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, listed: usize, went: Option<(u32, Hit)>) -> Value {
    let record = access.with(|s| s.viewing().find().clone());
    // Hits from a document the person opened with a password are text read with it: none of them goes to an agent, whichever tab is being read now.
    let mut about = None;
    finding.with_results(&mut |results| about = results.tab);
    let withheld = about.is_some_and(|tab| access.with(|s| s.withheld_from_agents(TabId(tab))));
    let mut value = json!(null);
    finding.with_results(&mut |results| {
        let state = match &results.end {
            _ if results.tab.is_none() => "idle",
            None => "running",
            Some(End::Finished) => "done",
            Some(End::Stopped) => "stopped",
            Some(End::OutOfTime(_)) => "out_of_time",
            Some(End::Failed(_)) => "failed",
        };
        let hit_json = |index: usize, hit: &Hit| json!({ "index": index, "page": hit.page + 1, "start": hit.start, "end": hit.end, "hidden": hit.hidden, "untrusted": { "before": hit.before, "matched": hit.matched, "after": hit.after } });
        let p = results.progress;
        value = json!({
            "open": record.open,
            "query": record.query,
            "tab": results.tab,
            "state": state,
            "status": results.status(),
            "hits_total": p.total,
            "hits_listed": results.hits.len(),
            "pages_with_hits": p.pages_with_hits,
            "pages_searched": p.pages_done,
            "pages_in_scope": p.pages,
            "failed_pages": p.failed,
            "no_text_pages": p.no_text,
            "first_hit_ms": results.first_hit.map(|took| took.as_secs_f64() * 1000.0),
            "took_ms": results.took.map(|took| took.as_secs_f64() * 1000.0),
            "current": results.current.and_then(|at| results.hits.get(at).map(|hit| hit_json(at, hit))),
            "hits": results.hits.iter().take(listed).enumerate().map(|(at, hit)| hit_json(at, hit)).collect::<Vec<_>>(),
        });
        if withheld {
            value = json!({ "open": record.open, "tab": results.tab, "state": state, "protected": true, "protected_note": crate::PROTECTED_NOTE, "hits": [] });
        }
    });
    if let Some((tab, hit)) = went {
        value["went_to"] = json!({ "tab": tab, "page": hit.page + 1, "start": hit.start, "end": hit.end });
    }
    value
}
