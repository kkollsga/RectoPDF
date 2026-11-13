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
        let start = || {
            let disk = Disk::new();
            let files: Vec<_> = pdfs.iter().map(|(name, pages)| disk.pdf(name, *pages)).collect();
            let root = disk.files().display().to_string();
            let app = disk.start();
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
        self.person.act(action);
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
    /// of the comparison, which the caller checks itself (who a jump was by).
    pub fn assert_same(&self, what: &str, except: &[&str]) {
        let shown = |app: &HeadlessApp, root: &str| format!("{:?} {:?}", app.reader(), app.sidebar()).replace(root, "<files>");
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
