//! **The person's action and the agent's command, side by side** (step R2.3): two headless apps on the same documents, one where the person does something at
//! the window ([`HeadlessApp::act`]) and one where an agent at Full sends the command it now is. What each published, announced and kept in the record is
//! compared, so a person's action that runs as a command is shown to do what that command does for an agent, and no more.

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Event, Level};
use serde_json::Value;

/// Two apps, each on its own disk with the same files, opened in the same order, so their tabs have the same numbers.
pub struct Twins {
    pub person: HeadlessApp,
    pub agent: HeadlessApp,
    roots: [String; 2],
}

impl Twins {
    /// Both apps, with these PDFs (name, pages) open, settled, and hearing what's announced from here on.
    pub fn on(pdfs: &[(&str, usize)]) -> Self {
        let files: Vec<(&str, Vec<u8>)> = pdfs
            .iter()
            .map(|(name, pages)| {
                let pages: Vec<recto_testpdf::Page> = (1..=*pages)
                    .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{name} page {page}")])))
                    .collect();
                (*name, recto_testpdf::pages(&pages))
            })
            .collect();
        Self::with(&files)
    }

    /// Both apps, with these files (name, bytes) open, settled, and hearing what's announced from here on.
    pub fn with(files: &[(&str, Vec<u8>)]) -> Self {
        Self::started(files, Disk::start)
    }

    /// [`Self::with`], each app as the app is with its state folder, with indexing and generated contents, as `start` makes it.
    pub fn with_generating(files: &[(&str, Vec<u8>)]) -> Self {
        Self::started(files, |disk| disk.builder().indexing(true).start())
    }

    fn started(files: &[(&str, Vec<u8>)], begin: fn(Disk) -> HeadlessApp) -> Self {
        let start = || {
            let disk = Disk::new();
            let files: Vec<_> = files.iter().map(|(name, bytes)| disk.file(name, bytes)).collect();
            let root = disk.files().display().to_string();
            let app = begin(disk);
            for file in &files {
                app.open(file);
            }
            app.hear_events();
            (app, root)
        };
        let ((person, a), (agent, b)) = (start(), start());
        Self { person, agent, roots: [a, b] }
    }

    /// The person does `action` at the window, and an agent at Full sends `line`; both settle. What each announced: the agent's what its command made, which
    /// the socket's server announces, and what the session thread did after.
    pub fn both(&self, action: UiAction, line: &str) -> (Vec<Event>, Vec<Event>) {
        self.both_with(
            |app| {
                app.act(action);
            },
            line,
        )
    }

    /// [`Self::both`], with what the person does said as a call on their app: a click on a link, which the page view reports and isn't a [`UiAction`].
    pub fn both_with(&self, person: impl FnOnce(&HeadlessApp), line: &str) -> (Vec<Event>, Vec<Event>) {
        person(&self.person);
        self.person.settle();
        let (result, mut said) = self.agent.agent_heard(Level::Full, line);
        if let Err(error) = result {
            panic!("the agent's `{line}` was refused: {error:?}");
        }
        self.agent.settle();
        said.extend(self.agent.heard_events());
        (self.person.heard_events(), said)
    }

    /// **Both apps show and keep the same:** the reader's model as the window pulls it, the sidebar's, and the record as it would be written, each with its
    /// disk's folder and its files' stamps (`file`: their inodes and times, which two disks never share) taken out. `except` names more record keys left out
    /// of the comparison, and fields of the window's models that say yes or no, which the caller checks itself (who a jump or an edit was by).
    pub fn assert_same(&self, what: &str, except: &[&str]) {
        let shown = |app: &HeadlessApp, root: &str| {
            let mut shown = format!("{:?} {:?}", app.reader(), app.sidebar()).replace(root, "<files>");
            for key in except {
                shown = shown
                    .replace(&format!("{key}: true"), &format!("{key}: _"))
                    .replace(&format!("{key}: false"), &format!("{key}: _"));
            }
            shown
        };
        assert_eq!(
            shown(&self.person, &self.roots[0]),
            shown(&self.agent, &self.roots[1]),
            "{what}: the window shows the same"
        );
        let zoom = |app: &HeadlessApp| {
            let inner = app.core();
            inner
                .panes
                .engines(&inner.engine)
                .iter()
                .map(recto_reader::Engine::magnification)
                .collect::<Vec<_>>()
        };
        assert_eq!(zoom(&self.person), zoom(&self.agent), "{what}: the pages are zoomed the same");
        let kept = |app: &HeadlessApp, root: &str| {
            let mut record: Value = serde_json::from_str(&app.session_json().to_string().replace(root, "<files>")).expect("json");
            strip(&mut record, &[&["file"], except].concat());
            record
        };
        assert_eq!(
            kept(&self.person, &self.roots[0]),
            kept(&self.agent, &self.roots[1]),
            "{what}: the record keeps the same"
        );
    }

    /// [`Self::assert_same`]'s comparison, said rather than asserted, short: where the two apps' window models differ (their fields, as `Debug` prints them),
    /// whether their pages are zoomed the same, and the record's keys that differ. Empty when they show and keep the same. For a test that lists every
    /// difference it finds before it fails.
    pub fn differences(&self, except: &[&str]) -> Vec<String> {
        let mut found = Vec::new();
        let fields = |app: &HeadlessApp, root: &str| {
            let mut shown = format!("{:?} {:?}", app.reader(), app.sidebar()).replace(root, "<files>");
            for key in except {
                shown = shown
                    .replace(&format!("{key}: true"), &format!("{key}: _"))
                    .replace(&format!("{key}: false"), &format!("{key}: _"));
            }
            shown.split(", ").map(str::to_string).collect::<Vec<_>>()
        };
        let (person, agent) = (fields(&self.person, &self.roots[0]), fields(&self.agent, &self.roots[1]));
        let shown: Vec<String> = person
            .iter()
            .zip(&agent)
            .filter(|(person, agent)| person != agent)
            .take(4)
            .map(|(person, agent)| format!("person `{person}`, agent `{agent}`"))
            .collect();
        if !shown.is_empty() || person.len() != agent.len() {
            found.push(format!("the window shows otherwise: {}", shown.join("; ")));
        }
        let zoom = |app: &HeadlessApp| {
            let inner = app.core();
            inner
                .panes
                .engines(&inner.engine)
                .iter()
                .map(recto_reader::Engine::magnification)
                .collect::<Vec<_>>()
        };
        if zoom(&self.person) != zoom(&self.agent) {
            found.push(format!("zoomed otherwise: person {:?}, agent {:?}", zoom(&self.person), zoom(&self.agent)));
        }
        let kept = |app: &HeadlessApp, root: &str| {
            let mut record: Value = serde_json::from_str(&app.session_json().to_string().replace(root, "<files>")).expect("json");
            strip(&mut record, &[&["file"], except].concat());
            record
        };
        let mut paths = Vec::new();
        differing("", &kept(&self.person, &self.roots[0]), &kept(&self.agent, &self.roots[1]), &mut paths);
        if !paths.is_empty() {
            found.push(format!("the record keeps otherwise at {}", paths.join(", ")));
        }
        found
    }
}

/// The paths where two records differ, the first few.
fn differing(at: &str, person: &Value, agent: &Value, paths: &mut Vec<String>) {
    if paths.len() >= 6 || person == agent {
        return;
    }
    match (person, agent) {
        (Value::Object(one), Value::Object(other)) => {
            for key in one.keys().chain(other.keys().filter(|key| !one.contains_key(*key))) {
                differing(
                    &format!("{at}.{key}"),
                    one.get(key).unwrap_or(&Value::Null),
                    other.get(key).unwrap_or(&Value::Null),
                    paths,
                );
            }
        }
        _ => {
            let short = |value: &Value| value.to_string().chars().take(100).collect::<String>();
            paths.push(format!("{at} (person {}, agent {})", short(person), short(agent)));
        }
    }
}

/// Takes these keys out, wherever they are.
fn strip(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Object(map) => {
            map.retain(|key, _| !keys.contains(&key.as_str()));
            map.values_mut().for_each(|value| strip(value, keys));
        }
        Value::Array(list) => list.iter_mut().for_each(|value| strip(value, keys)),
        _ => {}
    }
}

/// The names of the events, in order, for a message.
pub fn names(events: &[Event]) -> Vec<&'static str> {
    events.iter().map(Event::name).collect()
}
