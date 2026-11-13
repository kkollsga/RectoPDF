//! **What a window does after a command,** that nothing in the session says (step R2.3): a command declares it, for whoever asked, and a host with a window
//! carries it out. A session with no window has nothing to show, and hears none. Never on the wire: what watchers hear is [`crate::Event`].

use recto_session::TabId;

/// One thing for the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Find went to a hit: the page view selects it and brings it into view, as ⌘G's does.
    Reveal { tab: TabId, hit: crate::find::Hit },
    /// The person's step to a hit not known yet, as their query still runs: kept, and taken when it's known, or dropped when the query ends without it.
    FindStepPending { forward: bool, going: crate::find::Going },
}
