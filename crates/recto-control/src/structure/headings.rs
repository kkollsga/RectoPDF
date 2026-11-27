//! The structure helper's `headings` job, as the session reads it: one line a page that has headings and one end line, each believed only as far as it is checked (`docs/engineering/generated-contents.md`).
//!
//! **What a line may be.** A page line is exactly `{"page","found"}`: a page after the last one and before the claimed total's cap, with at least one entry, each exactly `{"level","bbox","title"}`
//! (a level from 1 to 6, a box that's null or four numbers that fit an `f32`, a title of at most `MAX_TITLE_BYTES` that has a letter or a digit once cleaned), and the entries in all no more than the
//! limit. The end is `{"state":"ready", …}` with the pages claimed in range and above every page line, the failed pages no more than the pages, `truncated` a bool, `found` equal to the entries
//! received, and nothing else but numbers and bools. A `refused`, `changed` or `failed` is only ever the first thing said. Anything else is bad, and the helper is killed.

use super::{Answer, Said};
use recto_index::generated::visible::{Found, Headings};
use recto_index::generated::{MAX_PAGES, MAX_TITLE_BYTES, short_title};
use serde_json::{Map, Value, json};

/// The job line: the headings of the whole document, the helper watching its own footprint against [`super::MEMORY_LIMIT`] as a second line.
pub fn job() -> Value {
    json!({ "kind": "headings", "memory_limit": super::MEMORY_LIMIT })
}

/// A line it won't have.
const BAD: &str = "bad_answer";
/// Most numbers and bools of the end line kept for measuring.
const MAX_NUMBERS: usize = 32;

/// What the helper has said of a document's headings, as far as it's been believed.
#[derive(Debug)]
pub struct HeadingsAnswer {
    max_entries: usize,
    found: Vec<Found>,
    /// The last page line's page.
    last_page: Option<u32>,
    /// Lines taken so far, so a refusal is only ever the first.
    lines: u32,
    /// The end line, once it's been believed.
    end: Option<End>,
}

#[derive(Debug)]
struct End {
    pages: u32,
    pages_failed: u32,
    truncated: bool,
    numbers: Map<String, Value>,
}

impl HeadingsAnswer {
    /// An answer that takes at most `max_entries` entries: `recto_index::generated::MAX_ENTRIES` in use.
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            found: Vec::new(),
            last_page: None,
            lines: 0,
            end: None,
        }
    }

    /// What was found, once the end has been believed.
    pub fn into_headings(self) -> Headings {
        let end = self.end;
        Headings {
            pages: end.as_ref().map_or(0, |end| end.pages),
            found: self.found,
            pages_failed: end.as_ref().map_or(0, |end| end.pages_failed),
            truncated: end.as_ref().is_some_and(|end| end.truncated),
        }
    }

    fn page_line(&mut self, line: &Map<String, Value>) -> Result<Said, &'static str> {
        if line.len() != 2 {
            return Err(BAD);
        }
        let page = line["page"].as_u64().and_then(|page| u32::try_from(page).ok()).filter(|page| *page < MAX_PAGES);
        let page = page.filter(|page| self.last_page.is_none_or(|last| *page > last)).ok_or(BAD)?;
        let entries = line.get("found").and_then(Value::as_array).filter(|entries| !entries.is_empty()).ok_or(BAD)?;
        if self.found.len() + entries.len() > self.max_entries {
            return Err(BAD);
        }
        let mut taken = Vec::with_capacity(entries.len());
        for entry in entries {
            taken.push(entry_of(entry, page).ok_or(BAD)?);
        }
        self.found.extend(taken);
        self.last_page = Some(page);
        Ok(Said::More)
    }

    fn end_line(&mut self, line: &Map<String, Value>) -> Result<Said, &'static str> {
        let number = |name: &str| line.get(name).and_then(Value::as_u64).and_then(|value| u32::try_from(value).ok());
        let pages = number("pages").filter(|pages| (1..=MAX_PAGES).contains(pages)).ok_or(BAD)?;
        let pages_failed = number("pages_failed").filter(|failed| *failed <= pages).ok_or(BAD)?;
        let truncated = line.get("truncated").and_then(Value::as_bool).ok_or(BAD)?;
        let counted = line.get("found").and_then(Value::as_u64).ok_or(BAD)?;
        let all_before = self.last_page.is_none_or(|last| last < pages);
        let plain = line.len() <= MAX_NUMBERS && line.iter().all(|(name, value)| name == "state" || value.is_number() || value.is_boolean());
        // The session's own measurements; a helper doesn't get to say them.
        let own = super::SESSION_NUMBERS.iter().any(|name| line.contains_key(*name));
        if counted != self.found.len() as u64 || !all_before || !plain || own {
            return Err(BAD);
        }
        // Only numbers go on, for measuring. Nothing the helper wrote is repeated as words.
        let numbers = line
            .iter()
            .filter(|(_, value)| value.is_number() || value.is_boolean())
            .map(|(name, value)| (recto_core::clean_line(name).chars().take(40).collect(), value.clone()))
            .collect();
        self.end = Some(End {
            pages,
            pages_failed,
            truncated,
            numbers,
        });
        Ok(Said::End)
    }

    /// A refusal, a change or a failure: the only things said instead of an answer, and only as the first.
    fn instead(&self, state: Option<&str>, line: &Map<String, Value>) -> Result<Said, &'static str> {
        if self.lines != 0 {
            return Err(BAD);
        }
        match state {
            Some("refused") => {
                let code = line.get("code").and_then(Value::as_str).unwrap_or("");
                let known = ["needs_password", "couldnt_read", "no_pages"].into_iter().find(|known| *known == code);
                Ok(Said::Refused(known.unwrap_or("couldnt_read").to_string()))
            }
            Some("changed") => Ok(Said::Changed),
            // What it failed at is its own word for it, never repeated.
            Some("failed") => Ok(Said::Failed("helper_stopped".into())),
            _ => Err(BAD),
        }
    }
}

/// One entry of a page line, as believed.
fn entry_of(entry: &Value, page: u32) -> Option<Found> {
    let entry = entry.as_object().filter(|entry| entry.len() == 3)?;
    let level = entry.get("level")?.as_u64().filter(|level| (1..=6).contains(level))? as u8;
    let bbox = match entry.get("bbox")? {
        Value::Null => None,
        Value::Array(edges) if edges.len() == 4 => {
            let mut taken = [0f32; 4];
            for (taken, edge) in taken.iter_mut().zip(edges) {
                *taken = edge.as_f64().map(|edge| edge as f32).filter(|edge| edge.is_finite())?;
            }
            Some(taken)
        }
        _ => return None,
    };
    let title = entry.get("title")?.as_str().filter(|title| title.len() <= MAX_TITLE_BYTES)?;
    let title = short_title(title);
    title.chars().any(char::is_alphanumeric).then_some(Found { level, page, bbox, title })
}

impl Answer for HeadingsAnswer {
    fn take(&mut self, line: &[u8]) -> Result<Said, &'static str> {
        let value: Value = serde_json::from_slice(line).map_err(|_| BAD)?;
        let line = value.as_object().ok_or(BAD)?;
        let said = if line.contains_key("page") {
            self.page_line(line)
        } else {
            match line.get("state").and_then(Value::as_str) {
                Some("ready") => self.end_line(line),
                state => self.instead(state, line),
            }
        };
        self.lines += 1;
        said
    }

    fn numbers(&self) -> Value {
        self.end.as_ref().map_or(Value::Null, |end| Value::Object(end.numbers.clone()))
    }
}

/// What the whole of an answer comes to, read from bytes the way the live reader reads them (a hello, page lines, an end), for the fuzz target and the tests. `Err` is why it wasn't
/// believed: the reader's own `bad_answer`, or a helper that said it refused, changed or failed, or stopped with no end.
pub fn read_all(said: &[u8], max_entries: usize) -> Result<(Headings, Value), &'static str> {
    let (answer, end, bad) = super::read_bytes(said, HeadingsAnswer::new(max_entries));
    if bad {
        return Err(BAD);
    }
    match end {
        Some(Said::End) => {
            let numbers = answer.numbers();
            Ok((answer.into_headings(), numbers))
        }
        Some(Said::Refused(_)) => Err("refused"),
        Some(Said::Changed) => Err("changed"),
        _ => Err("helper_stopped"),
    }
}

#[cfg(test)]
mod tests;
