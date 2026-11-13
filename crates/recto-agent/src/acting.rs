//! **Whose command is changing the session, said to it around each look the one executor takes** (the sleep branch's second review, 2026-11-13). A command
//! takes the session's lock for a look at a time, and the person's commands and an agent's can run between each other's looks, so who's acting is said
//! inside each look and taken back after it, never for a whole command. What it decides today: a tab an agent's command pushes out of the warm ones has its
//! sleep file counted against agents' share of the hour, by whatever route the command came (AGENTS: "An agent's action is an agent's by whatever route it
//! arrives").

use crate::{Actor, SessionAccess, commands, find, generated, index, later, saving};
use recto_session::{Author, Session};

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

impl<A: SessionAccess> SessionAccess for Acting<'_, A> {
    type Loader = A::Loader;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Self::Loader>) -> R) -> R {
        self.access.with(|session| {
            session.dormancy_mut().act_as(self.by);
            let result = step(session);
            session.dormancy_mut().act_as(Author::Owner);
            result
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
