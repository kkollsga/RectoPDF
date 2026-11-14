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
                    lock(&inner.tools.prompts().passwords).failed(ask.id);
                    inner.window.notices().fail_open(passwords::CANT_KEEP.to_string());
                }
                Some(identity) => {
                    lock(&inner.tools.prompts().passwords).opened(ask.id);
                    inner.keys.set(identity, password);
                    for engine in inner.panes.engines(&inner.engine) {
                        engine.key_given(&ask.path);
                    }
                    match ask.target {
                        passwords::Target::File => match host.session.with_session(|session| session.open_unlocked(&ask.path, facts, stamp)) {
                            Ok(id) => events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id))),
                            Err(error) => inner.window.notices().fail_open(error.message().to_string()),
                        },
                        passwords::Target::Tab(id) => host.session.with_session(|session| session.unlock(id, facts, stamp)),
                    }
                }
            }
        }
        Err(OpenError::PasswordNeeded) => {
            if lock(&inner.tools.prompts().passwords).wrong(ask.id) == passwords::Wrong::Stopped {
                inner.window.notices().say(passwords::stopped_asking());
            }
        }
        Err(error) => {
            lock(&inner.tools.prompts().passwords).failed(ask.id);
            inner.window.notices().fail_open(error.message().to_string());
        }
    }
}

/// **The password sheet's answer, as the window gives it,** before the session thread takes what comes of it: for the prompt with this id, with what was typed, or
/// nothing for Cancel. Tried on a thread of its own, as reading a file can wait on its disk. The prompts' own, with the session thread's queue and the window,
/// whose notice says when the asking has stopped.
impl crate::prompts::Prompts {
    pub fn password_answered(&self, id: u64, password: Option<String>, core: &Arc<CoreQueue>, window: &Window) {
        let mut asks = lock(&self.passwords);
        let Some(typed) = password else {
            asks.cancel(id);
            drop(asks);
            core.push(CoreCall::Reconcile);
            return;
        };
        let password = recto_core::Password::take(typed);
        let Some(password) = password else {
            // Not something a password could be, which counts as a wrong one.
            if asks.wrong(id) == passwords::Wrong::Stopped {
                window.notices().say(passwords::stopped_asking());
            }
            drop(asks);
            core.push(CoreCall::Reconcile);
            return;
        };
        let Some(asked) = asks.start_trying(id) else { return };
        drop(asks);
        let core_for_try = Arc::clone(core);
        let password = Arc::new(password);
        let spawned = std::thread::Builder::new().name("recto-unlock".into()).stack_size(512 * 1024).spawn(move || {
            let stamp = recto_platform::file_stamp(&asked.path).ok();
            let loaded = recto_core::inspect_with_password(&asked.path, &password);
            core_for_try.push(CoreCall::Unlocked(asked, loaded, stamp, password));
        });
        if spawned.is_err() {
            lock(&self.passwords).failed(id);
        }
        core.push(CoreCall::Reconcile);
    }
}
