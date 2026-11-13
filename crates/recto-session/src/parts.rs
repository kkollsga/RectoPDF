//! **The session's components, as callers reach them.** A caller that needs one concern takes that component, not the whole session. What's `pub` on a
//! component can't leave it out of step with another one: what has to change two together stays `pub(crate)`, called only by the session's own
//! operations. So a `&mut` here goes out only for a component a caller outside the crate changes today.

use super::*;

impl<L: DocLoader> Session<L> {
    /// The view's settings, Find and edit mode.
    pub fn viewing(&self) -> &viewing::Viewing {
        &self.viewing
    }

    pub fn viewing_mut(&mut self) -> &mut viewing::Viewing {
        &mut self.viewing
    }
}
