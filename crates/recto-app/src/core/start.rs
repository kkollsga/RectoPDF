//! **Starting the app's core from what its host gives it** (step R1.3): the listener, the clock, the state folder, the launch's arguments, where updates come from, and
//! whether there's a control socket and a search index. The bridge gives the real ones; a test gives fakes ([`crate::testkit::headless`]), and both get the same core.

use super::*;
use crate::clock::Clock;
use crate::launch::LaunchArgs;
use recto_control::record_store::{Flavor, RecordStore};
use recto_reader::EngineConfig;
use recto_render_service::Policy;

/// What the host starts the core with.
pub struct Start {
    pub listener: Arc<dyn crate::Listener>,
    /// What the saver's and the sleeper's decisions go by.
    pub clock: Arc<dyn Clock>,
    /// The state folder: the record, the backups, the index and the socket are in it.
    pub paths: StatePaths,
    pub args: LaunchArgs,
    pub update_source: recto_update::source::Source,
    /// Whether the control socket is served. The app's always is; a test's core has none unless it asks.
    pub socket: bool,
    /// Whether the search index and generated contents are there. The app's always are.
    pub indexing: bool,
}

impl Inner {
    /// **Joins the session and the socket.** Binds the socket, which is quick, and starts the session thread. A socket another session holds doesn't stop a normal run:
    /// the reader works, and `recto` keeps talking to that session. The page views' hooks are the core's own, onto its queue and the listener.
    ///
    /// # Errors
    ///
    /// Why it couldn't start, in words: a quiet run's socket is taken, or the session thread couldn't be started.
    pub fn start(start: Start) -> Result<Arc<Self>, String> {
        Self::start_with(start, updates::UpdateSeams::default())
    }

    /// [`Self::start`], with the updater's wall clock and releases and what starts the installer of the caller's: a test's ([`crate::testkit::update_bed`]).
    ///
    /// # Errors
    ///
    /// As [`Self::start`].
    pub fn start_with(start: Start, update_seams: updates::UpdateSeams) -> Result<Arc<Self>, String> {
        let Start {
            listener,
            clock,
            paths,
            args,
            update_source,
            socket,
            indexing,
        } = start;
        let server = if socket {
            match Server::bind(&paths) {
                Ok(server) => Some(server),
                Err(error) if args.qa => return Err(AgentError::from(error).message),
                Err(_) => None,
            }
        } else {
            None
        };
        let core = Arc::new(CoreQueue::default());
        let hooks: Arc<dyn EngineEvents> = Arc::new(EngineHooks {
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
        });
        let mut policy = Policy::default();
        if let Some(limit) = args.memory_limit_mib {
            policy.memory_limit_bytes = limit * 1024 * 1024;
            // At or below the normal limit, this step is off, so a page reaches its lower-resolution version straight away.
            policy.heavy_limit_bytes = policy.memory_limit_bytes;
        }
        let keys = Arc::new(recto_reader::engine::Keyring::default());
        let config = EngineConfig {
            service: None,
            policy: Some(policy),
            keys: Some(Arc::clone(&keys)),
        };
        let engine = Engine::new(config.clone(), Arc::clone(&hooks));
        let watchdog = Watchdog::new();
        let qa = args
            .qa
            .then(|| Arc::new(QaBridge::new(Arc::clone(&listener), engine.clone(), watchdog.clone(), Arc::clone(&core))));
        // Takes the record's lock, and reads nothing: that's `begin_restore`'s, on a thread that can wait for it.
        let saver = Arc::new(Saver::new(RecordStore::open(&paths, Flavor::App), Arc::clone(&core), Arc::clone(&clock)));
        let session = if indexing {
            SessionHost::new(DrawableLoader).with_indexing(&paths).with_generating(&paths)
        } else {
            SessionHost::new(DrawableLoader)
        };
        let host = Arc::new(AppHost {
            engine: engine.clone(),
            session,
            core: Arc::clone(&core),
            qa: qa.clone(),
            saver: Arc::clone(&saver),
            running: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
            watchers: Mutex::new(None),
        });
        host.listen();
        // The person's own Find reads a document they opened with a password through its key, as the page views draw it.
        let find_keys = Arc::clone(&keys);
        host.session.set_find_keys(Arc::new(move |path: &std::path::Path| find_keys.for_path(path)));
        let backups = recto_control::host::backups_folder(&paths);
        recto_core::save::prune_backups(&backups, std::time::SystemTime::now());
        let save_step = Arc::new(AtomicU8::new(0));
        host.session.set_saving(Arc::new(saving::AppSaving {
            saver: Arc::clone(&saver),
            backups,
            step: Arc::clone(&save_step),
            core: Arc::clone(&core),
        }));
        host.session.set_records(Arc::new(super::record_commands::AppRecords(Arc::downgrade(&host))));
        let stopper = server.as_ref().and_then(|server| server.stopper().ok());
        let announcer = server.as_ref().and_then(|server| server.announcer().ok());
        let update_hooks = updates::Hooks::new(update_seams.installer);
        let updates = recto_update::updater::Updater::with_seams(
            &paths.root,
            update_source,
            Arc::clone(&update_hooks) as Arc<dyn recto_update::updater::Host>,
            update_seams.updater,
        );
        let inner = Arc::new(Self {
            updates,
            host: Arc::clone(&host),
            engine,
            panes: panes::Panes::new(hooks, config),
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
            published: Mutex::new(Published::empty()),
            saver: Arc::clone(&saver),
            sleeper: sleeper::Sleeper::new(Arc::clone(&clock)),
            qa,
            keys,
            life: Lifecycle::new(clock),
            serving: Serving {
                paths: Some(paths),
                socket,
                waiting_server: Mutex::new(server),
                stopper: Mutex::new(stopper),
                announcer: Mutex::new(announcer),
            },
            window: WindowParts::new(watchdog),
            tools: Tools::new(&save_step),
        });
        saver.attach(Arc::downgrade(&inner));
        update_hooks.attach(Arc::downgrade(&inner));
        if let Some(qa) = &inner.qa {
            qa.attach(Arc::downgrade(&inner));
        }
        let session_side = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("recto-session".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || session_thread(&session_side))
            .map_err(|_| "The app couldn't start its session thread.".to_string())?;
        if args.qa {
            let state = inner.serving.paths.as_ref().map(|paths| paths.root.clone());
            qa::end_with_starter(args.time_limit.unwrap_or(qa::QUIET_RUN_LIMIT), args.owner_pid, state, Arc::downgrade(&inner));
        }
        Ok(inner)
    }
}
