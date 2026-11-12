//! **A protected file's password, as the session thread takes the answer:** the file opened or unlocked with it and the key kept, or the try counted.

use super::*;

/// `CoreCall::Unlocked`, on the session thread.
pub fn unlocked(
    inner: &Arc<Inner>,
    ask: crate::passwords::Ask,
    loaded: Result<DocFacts, OpenError>,
    stamp: Option<FileStamp>,
    password: Arc<recto_core::Password>,
    events: &mut Vec<Event>,
) {
    let host = &inner.host;
    match loaded {
        Ok(facts) => {
            // Kept by which file it is, so a move keeps it and a new file in its place doesn't get it. A file with no such numbers can't have its password kept for drawing it, so it's said rather than asked again for ever.
            let identity = stamp.map(|stamp| (stamp.device, stamp.inode)).filter(|identity| *identity != (0, 0));
            match identity {
                None => {
                    lock(&inner.tools.prompts.passwords).failed(ask.id);
                    *lock(&inner.window.open_failure) = Some(passwords::CANT_KEEP.to_string());
                }
                Some(identity) => {
                    lock(&inner.tools.prompts.passwords).opened(ask.id);
                    inner.keys.set(identity, password);
                    for engine in inner.panes.engines(&inner.engine) {
                        engine.key_given(&ask.path);
                    }
                    match ask.target {
                        passwords::Target::File => match host.session.with_session(|session| session.open_unlocked(&ask.path, facts, stamp)) {
                            Ok(id) => events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id))),
                            Err(error) => *lock(&inner.window.open_failure) = Some(error.message().to_string()),
                        },
                        passwords::Target::Tab(id) => host.session.with_session(|session| session.unlock(id, facts, stamp)),
                    }
                }
            }
        }
        Err(OpenError::PasswordNeeded) => {
            if lock(&inner.tools.prompts.passwords).wrong(ask.id) == passwords::Wrong::Stopped {
                *lock(&inner.window.action_notice) = Some(passwords::stopped_asking());
            }
        }
        Err(error) => {
            lock(&inner.tools.prompts.passwords).failed(ask.id);
            *lock(&inner.window.open_failure) = Some(error.message().to_string());
        }
    }
}
