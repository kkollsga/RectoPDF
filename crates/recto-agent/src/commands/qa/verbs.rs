//! **The QA verb table** (step R2.4): every verb the app answers, what kind of verb it is, and which of the host's areas answers it, if any. Each verb's
//! name, level and help are its row in [`super::ROWS_QA`]; this says the rest, one line a verb, beside the rows it belongs to. Both sides are held to it:
//! the app's dispatch in Rust refuses a verb that isn't here and never sends the host one the host doesn't answer, and
//! [`tests::swift_answers_exactly_the_tables_verbs`] reads the Swift switches and fails on any verb they answer that isn't here, or here that they
//! don't, or in two areas.
//!
//! The guide's §2.9: a test verb is for what only the host has. A verb that would do what a command does has no kind here: a test sends the command,
//! as an agent at its level, or as the person through `as_person`.

use crate::table::Spec;

/// What a verb is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QaKind {
    /// Reads what the app shows, and changes nothing: the report, a picture, the clipboard.
    Look,
    /// What only AppKit can do and no command says: the pointer, a key in a field, a menu chosen through AppKit, a sheet's button, the window itself.
    HostInput,
    /// A test's lever on the machine around the app: the system's settings, a stall, a slow seam, a clock, what a record holds that no action makes.
    Condition,
    /// A `recto` command run as the person at the window, for what only a person may do: the owner's edits, the window's Find, an action that doesn't
    /// wait for its read. One verb, `as_person`, at Full, on a test's own files.
    Person,
    /// Rust's own step on the way to a verb that has a row, never sent by a client: no row.
    Internal,
}

/// The host's areas, one Swift file each: `app/Sources/RectoPDF/QAChannel+<Area>.swift`, whose `qa<Area>` answers that area's verbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Pointer,
    Window,
    Controls,
    Sidebar,
    Find,
    Panes,
    Menus,
    Dialogs,
    Shots,
    Report,
}

impl Area {
    pub const ALL: [Self; 10] = [
        Self::Pointer,
        Self::Window,
        Self::Controls,
        Self::Sidebar,
        Self::Find,
        Self::Panes,
        Self::Menus,
        Self::Dialogs,
        Self::Shots,
        Self::Report,
    ];

    /// As the Swift file and its function name it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pointer => "Pointer",
            Self::Window => "Window",
            Self::Controls => "Controls",
            Self::Sidebar => "Sidebar",
            Self::Find => "Find",
            Self::Panes => "Panes",
            Self::Menus => "Menus",
            Self::Dialogs => "Dialogs",
            Self::Shots => "Shots",
            Self::Report => "Report",
        }
    }
}

/// One verb: its name on the wire, its kind, and the host's area that answers it, or `None` when only Rust does (the host never hears it).
#[derive(Clone, Copy, Debug)]
pub struct QaVerb {
    pub verb: &'static str,
    pub kind: QaKind,
    pub host: Option<Area>,
}

/// One line of the table.
pub(super) const fn verb(verb: &'static str, kind: QaKind, host: Option<Area>) -> QaVerb {
    QaVerb { verb, kind, host }
}

/// Rust's own steps, which no client sends.
const INTERNAL: &[QaVerb] = &[
    // What `qa wait-settled` and a shot ask until nothing is moving.
    verb("animations", QaKind::Internal, Some(Area::Report)),
    // One step of `qa sweep`, measured by the host.
    verb("scroll_step", QaKind::Internal, Some(Area::Pointer)),
];

/// The table, from each part beside its rows.
const PARTS: &[&[QaVerb]] = &[super::rows_window::VERBS, super::rows_features::VERBS, INTERNAL];

/// Every verb the app answers.
pub fn all() -> impl Iterator<Item = &'static QaVerb> {
    PARTS.iter().flat_map(|part| part.iter())
}

/// A verb's line in the table.
#[must_use]
pub fn find(verb: &str) -> Option<&'static QaVerb> {
    all().find(|line| line.verb == verb)
}

/// The row a client sends a verb by: the one whose fixed `verb` is it. An internal verb has none, so no client can send it.
#[must_use]
pub fn row_of(verb: &str) -> Option<&'static Spec> {
    super::ROWS_QA
        .iter()
        .flat_map(|rows| rows.iter())
        .find(|spec| spec.fixed.iter().any(|(key, value)| *key == "verb" && *value == verb))
}

/// Whether the host answers this verb, as Rust may ask it.
#[must_use]
pub fn host_answers(verb: &str) -> bool {
    find(verb).is_some_and(|line| line.host.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    /// The verb a row sends, if it's one of the app's.
    fn row_verb(spec: &Spec) -> Option<&'static str> {
        spec.fixed.iter().find(|(key, _)| *key == "verb").map(|(_, value)| *value)
    }

    /// Failure: a QA row is added, renamed or removed and the table isn't, or the other way, so a verb has no kind or no area, or a line in the table names
    /// a verb no client can send. Every row with a verb has exactly one line, and a line with no row is Rust's own step.
    #[test]
    fn every_qa_row_has_one_verb_and_back() {
        let mut seen = BTreeMap::new();
        for line in all() {
            assert!(seen.insert(line.verb, line.kind).is_none(), "`{}` is in the table twice", line.verb);
        }
        for spec in super::super::ROWS_QA.iter().flat_map(|rows| rows.iter()) {
            match (spec.tag, row_verb(spec)) {
                ("qa_read" | "qa_drive", Some(verb)) => assert!(seen.contains_key(verb), "`{}` sends `{verb}`, which isn't in the table", spec.name),
                ("qa_read" | "qa_drive", None) => panic!("`{}` is a QA row with no verb", spec.name),
                (_, Some(verb)) => panic!("`{}` is `recto`'s own and sends `{verb}`", spec.name),
                _ => {}
            }
        }
        for line in all() {
            let row = row_of(line.verb);
            match line.kind {
                QaKind::Internal => assert!(row.is_none(), "`{}` is internal and has a row a client could send", line.verb),
                _ => assert!(row.is_some(), "`{}` is in the table with no row", line.verb),
            }
        }
    }

    /// Failure: a QA verb is let in at the level of the first row of its command, not its own, so a verb that needs Full (`as-person`) is let through at
    /// Control, or a look needs more than Read-only. Every row's verb, sent as its command, needs what its row says.
    #[test]
    fn each_qa_verb_needs_its_own_rows_level() {
        for spec in super::super::ROWS_QA.iter().flat_map(|rows| rows.iter()) {
            let Some(verb) = row_verb(spec) else { continue };
            let line = serde_json::json!({ "cmd": spec.tag, "verb": verb, "value": "x" });
            let mut line = line;
            if spec.tag == "qa_read" {
                line.as_object_mut().unwrap().remove("value");
            }
            let command = crate::parse(&line.to_string()).unwrap_or_else(|error| panic!("{}: {error:?}", spec.name));
            assert_eq!(command.access(), spec.access, "{}", spec.name);
        }
        assert_eq!(row_of("as_person").map(|row| row.access), Some(crate::Access::At(crate::Level::Full)));
    }

    /// The Swift sources the host's QA is in.
    fn swift_folder() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/Sources/RectoPDF")
    }

    /// The string labels of `func qa<Area>`'s own `switch verb`, in order, from a Swift source. Only the switch's own `case` lines, at its indentation:
    /// a switch inside one of them is deeper. Anything that isn't a list of string literals, or a `default` that doesn't hand the verb on, is an error.
    fn area_verbs(source: &str, area: &str) -> Result<Vec<String>, String> {
        let mut lines = source.lines().skip_while(|line| !line.contains(&format!("func qa{area}(")));
        lines.next().ok_or_else(|| format!("no `func qa{area}(`"))?;
        let switch = lines.next().ok_or("nothing after the function")?;
        if switch.trim() != "switch verb {" {
            return Err(format!("`func qa{area}` doesn't start with `switch verb {{`: `{}`", switch.trim()));
        }
        let indent = switch.len() - switch.trim_start().len();
        let at_switch = |line: &str| line.len() > indent && line.len() - line.trim_start().len() == indent;
        let mut verbs = Vec::new();
        let mut handed_on = false;
        let mut after_default = false;
        for line in lines {
            if after_default {
                if line.trim() != "return nil" {
                    return Err(format!("`qa{area}`'s `default` answers something: `{}`", line.trim()));
                }
                handed_on = true;
                after_default = false;
                continue;
            }
            if !at_switch(line) {
                continue;
            }
            let code = line.trim();
            if code == "}" {
                break;
            }
            if code == "default:" {
                after_default = true;
                continue;
            }
            let labels = code
                .strip_prefix("case ")
                .and_then(|rest| rest.strip_suffix(':'))
                .ok_or_else(|| format!("`qa{area}`'s switch has `{code}`, which isn't a verb's case"))?;
            for label in labels.split(", ") {
                let verb = label
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .filter(|verb| !verb.is_empty() && verb.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
                    .ok_or_else(|| format!("`qa{area}` answers `{label}`, which isn't a verb's name"))?;
                verbs.push(verb.to_owned());
            }
        }
        if !handed_on {
            return Err(format!("`qa{area}`'s switch has no `default: return nil`, so the next area never hears a verb"));
        }
        Ok(verbs)
    }

    /// Each area's verbs, by the area file they're in.
    type ByArea = BTreeMap<String, Vec<String>>;

    /// What the host answers: each area's verbs, with the areas `QAChannel.handle` asks in turn.
    fn host_verbs(folder: &Path) -> Result<(ByArea, BTreeSet<String>), String> {
        let mut areas = BTreeMap::new();
        for entry in std::fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))? {
            let name = entry.map_err(|error| error.to_string())?.file_name().to_string_lossy().into_owned();
            let Some(area) = name.strip_prefix("QAChannel+").and_then(|rest| rest.strip_suffix(".swift")) else {
                continue;
            };
            let source = std::fs::read_to_string(folder.join(&name)).map_err(|error| error.to_string())?;
            areas.insert(area.to_owned(), area_verbs(&source, area).map_err(|error| format!("{name}: {error}"))?);
        }
        let channel = std::fs::read_to_string(folder.join("QAChannel.swift")).map_err(|error| error.to_string())?;
        let handle = channel
            .lines()
            .find(|line| line.contains("let finished = "))
            .ok_or("`QAChannel.handle` has no `let finished = ` chain of areas")?;
        let chained = handle
            .split("qa")
            .skip(1)
            .filter_map(|part| part.split_once("(id, verb, value, controller)").map(|(area, _)| area.to_owned()))
            .collect();
        Ok((areas, chained))
    }

    /// What's wrong between the table and what the host answers, in words, one line each: nothing when they agree.
    fn mismatches(areas: &ByArea, chained: &BTreeSet<String>) -> Vec<String> {
        let mut wrong = Vec::new();
        let known: BTreeSet<String> = Area::ALL.iter().map(|area| area.name().to_owned()).collect();
        for area in areas.keys() {
            if !known.contains(area) {
                wrong.push(format!("QAChannel+{area}.swift is an area the table doesn't have"));
            }
        }
        for area in &known {
            if !areas.contains_key(area) {
                wrong.push(format!("the table's area {area} has no QAChannel+{area}.swift"));
            }
        }
        if chained != &areas.keys().cloned().collect::<BTreeSet<_>>() {
            wrong.push(format!(
                "`QAChannel.handle` asks {chained:?}, and the area files are {:?}",
                areas.keys().collect::<Vec<_>>()
            ));
        }
        let mut answered: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (area, verbs) in areas {
            for verb in verbs {
                answered.entry(verb.as_str()).or_default().push(area.as_str());
            }
        }
        for (verb, found_in) in &answered {
            if found_in.len() > 1 {
                wrong.push(format!("`{verb}` is answered more than once, in {found_in:?}"));
            }
            match find(verb) {
                None => wrong.push(format!("Swift answers `{verb}` (in {}), which isn't in the table", found_in[0])),
                Some(QaVerb { host: None, .. }) => wrong.push(format!("Swift answers `{verb}` (in {}), which the table says only Rust answers", found_in[0])),
                Some(QaVerb { host: Some(area), .. }) if !found_in.contains(&area.name()) => {
                    wrong.push(format!("`{verb}` is answered in {found_in:?}, and the table says {}", area.name()));
                }
                Some(_) => {}
            }
        }
        for line in all() {
            if let Some(area) = line.host
                && !answered.contains_key(line.verb)
            {
                wrong.push(format!("the table says {} answers `{}`, and no Swift area does", area.name(), line.verb));
            }
        }
        wrong
    }

    /// Failure: the host answers a verb the table doesn't have (a verb added to Swift alone, which no row, level or check knows), or doesn't answer one it
    /// says the host does (a verb Rust sends that comes back "That isn't a QA command"), or answers one in two areas, or a new area file that `handle`
    /// never asks. Reads the Swift sources and starts nothing.
    #[test]
    fn swift_answers_exactly_the_tables_verbs() {
        let (areas, chained) = host_verbs(&swift_folder()).expect("the Swift QA sources");
        let wrong = mismatches(&areas, &chained);
        assert!(wrong.is_empty(), "The host's QA verbs and the table differ:\n{}", wrong.join("\n"));
    }

    /// Failure: the reading of the Swift switch misses what it's for, so the guard above passes with the host answering something else: a verb added,
    /// one taken out, one moved to a second area, a `default` that answers, a pattern that isn't a name, and a verb inside a nested switch counted.
    #[test]
    fn the_swift_reading_sees_what_it_is_for() {
        let area = |cases: &str| {
            format!("    func qaFind(_ id: UInt64) -> Bool? {{\n        switch verb {{\n{cases}        default:\n            return nil\n        }}\n    }}\n")
        };
        let source = area(
            "        case \"find_type\":\n            switch value {\n            case \"inner\": break\n            }\n            return true\n        case \"find_key\", \"find_click\":\n            return true\n",
        );
        assert_eq!(area_verbs(&source, "Find").unwrap(), ["find_type", "find_key", "find_click"]);
        assert!(
            area_verbs(&area("        case let other:\n            return true\n"), "Find").is_err(),
            "a pattern"
        );
        assert!(
            area_verbs(&source.replace("return nil", "return true"), "Find").is_err(),
            "a default that answers"
        );
        assert!(area_verbs(&source, "Pointer").is_err(), "the wrong function");
        let (mut areas, chained) = host_verbs(&swift_folder()).unwrap();
        assert!(mismatches(&areas, &chained).is_empty());
        let mut added = areas.clone();
        added.get_mut("Find").unwrap().push("find_everything".to_owned());
        assert!(
            mismatches(&added, &chained).iter().any(|line| line.contains("find_everything")),
            "a verb added to Swift alone"
        );
        let mut twice = areas.clone();
        twice.get_mut("Window").unwrap().push("find_type".to_owned());
        assert!(
            mismatches(&twice, &chained).iter().any(|line| line.contains("more than once")),
            "a verb in two areas"
        );
        areas.get_mut("Find").unwrap().retain(|verb| verb != "find_click");
        assert!(
            mismatches(&areas, &chained).iter().any(|line| line.contains("find_click")),
            "a verb the host stopped answering"
        );
        let mut unasked = chained.clone();
        unasked.remove("Find");
        assert!(!mismatches(&areas, &unasked).is_empty(), "an area `handle` never asks");
    }
}
