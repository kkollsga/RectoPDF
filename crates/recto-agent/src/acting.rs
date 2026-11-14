//! **Whose command is changing the session, said to it around each look the one executor takes** (the sleep branch's second review, 2026-11-13). A command
//! takes the session's lock for a look at a time, and the person's commands and an agent's can run between each other's looks, so who's acting is said
//! inside each look and taken back after it, never for a whole command. What it decides today: a tab an agent's command pushes out of the warm ones has its
//! sleep file counted against agents' share of the hour, by whatever route the command came (AGENTS: "An agent's action is an agent's by whatever route it
//! arrives").

use crate::{Actor, SessionAccess, commands, find, generated, index, later, saving};
use recto_session::{Author, DocLoader, Session};

/// A host's way to the session, with each look said to be `by`'s.
pub(crate) struct Acting<'a, A> {
    access: &'a A,
    by: Author,
}

impl<'a, A: SessionAccess> Acting<'a, A> {
    pub(crate) fn new(access: &'a A, actor: Actor) -> Self {
        Self { access, by: actor.author() }
    }
}

/// **One look at the session said to be someone's,** and the owner's again when it ends, by returning or by a panic: a step that panics is caught by
/// the server and the session's lock recovers from the poisoning, so without this the agent would stay acting for every change made outside a command
/// until anyone's next one (a restore, a read landing), and the person's pushed-out tabs would be counted against agents' share of sleep files.
struct Look<'s, L: DocLoader>(&'s mut Session<L>);

impl<'s, L: DocLoader> Look<'s, L> {
    fn by(session: &'s mut Session<L>, by: Author) -> Self {
        session.dormancy_mut().act_as(by);
        Self(session)
    }
}

impl<L: DocLoader> Drop for Look<'_, L> {
    fn drop(&mut self) {
        self.0.dormancy_mut().act_as(Author::Owner);
    }
}

impl<A: SessionAccess> SessionAccess for Acting<'_, A> {
    type Loader = A::Loader;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Self::Loader>) -> R) -> R {
        self.access.with(|session| {
            let look = Look::by(session, self.by);
            step(&mut *look.0)
        })
    }
    fn indexing(&self) -> Option<&dyn index::Indexing> {
        self.access.indexing()
    }
    fn saving(&self) -> Option<&dyn saving::SaveSupport> {
        self.access.saving()
    }
    fn generating(&self) -> Option<&dyn generated::Generating> {
        self.access.generating()
    }
    fn finding(&self) -> Option<&dyn find::Finding> {
        self.access.finding()
    }
    fn records(&self) -> Result<&dyn commands::record::Records, crate::AgentError> {
        self.access.records()
    }
    fn effect(&self, effect: crate::Effect) {
        self.access.effect(effect);
    }
    fn find_keys(&self, actor: Actor) -> Option<find::KeyLookup> {
        self.access.find_keys(actor)
    }
    fn reads_later(&self) -> Option<&dyn later::ReadLater> {
        self.access.reads_later()
    }
    fn answer_read(&self) -> bool {
        self.access.answer_read()
    }
    fn take_places(&self) {
        self.access.take_places();
    }
    fn confinement(&self) -> Option<std::sync::Arc<recto_platform::run_folders::RunFolders>> {
        self.access.confinement()
    }
}

#[cfg(test)]
mod tests {
    use super::Acting;
    use crate::{Actor, Level, SessionAccess};
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::{Author, DocLoader, MAX_WARM, Session};
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 2,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    /// **Failure: a step that panics inside an agent's look leaves the agent acting on the session.** The server catches the panic and the session's
    /// lock recovers from the poisoning, and every change made outside a command meanwhile (a restore, a read landing, the person's own opens) is
    /// counted as the agent's: once agents' share of sleep files is used, the person's pushed-out tabs get no file and wake bare.
    #[test]
    fn a_step_that_panics_leaves_the_owner_acting() {
        let session = Mutex::new(Session::new(Fake));
        session.with(|session| session.dormancy_mut().set_sleep_files(true));
        let agent = Acting::new(&session, Actor::Agent(Level::Full));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| agent.with(|_| panic!("a step's own bug"))));
        assert!(caught.is_err(), "the step panicked");

        // Outside any command: tabs opened past the warm ones push the first out. Real files, as a sleep file is named for its file's stamp.
        let folder = recto_testpdf::TempDir::new("recto-acting-");
        session.with(|session| {
            for n in 0..=MAX_WARM {
                let path = folder.path().join(format!("{n}.pdf"));
                std::fs::write(&path, b"%PDF-1.7").unwrap();
                session.open(&path).unwrap();
            }
        });
        let left: Vec<Author> = session
            .with(|session| session.sleeper().take_left_warm())
            .into_iter()
            .map(|(_, by)| by)
            .collect();
        assert!(!left.is_empty(), "a tab was pushed out");
        assert!(
            left.iter().all(|by| *by == Author::Owner),
            "pushed out outside any command, so the owner's: {left:?}"
        );
    }

    /// **Failure: a method added to [`SessionAccess`] with a default body isn't forwarded by [`Acting`],** so a host's own answer to it (its saving,
    /// its records, its passwords' rule) is silently replaced by the default for every command, whoever sends it. The trait's methods are read from its
    /// source, and each must be implemented here.
    #[test]
    fn acting_forwards_every_method_of_session_access() {
        let lib = include_str!("lib.rs");
        let start = lib.find("pub trait SessionAccess {").expect("the trait is in lib.rs");
        let body = &lib[start..];
        let end = body.find("\n}\n").expect("the trait's end");
        let methods: Vec<&str> = body[..end]
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("fn "))
            .map(|rest| rest.split(['<', '(']).next().unwrap_or(rest))
            .collect();
        assert!(methods.len() >= 10, "the trait's methods were read: {methods:?}");
        let acting = include_str!("acting.rs");
        let implemented = &acting[acting.find("impl<A: SessionAccess> SessionAccess for Acting").expect("the impl")..];
        let implemented = &implemented[..implemented.find("\n}\n").expect("the impl's end")];
        let missing: Vec<&&str> = methods.iter().filter(|name| !implemented.contains(&format!("fn {name}"))).collect();
        assert!(
            missing.is_empty(),
            "Acting doesn't forward {missing:?}: each SessionAccess method is forwarded to the host it wraps"
        );
    }
}
