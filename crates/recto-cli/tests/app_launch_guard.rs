//! **A test never starts the build folder's own app by hand.** Starting the built app registers it with the system, and a test that runs its executable itself ends with nothing that forgets the build folder afterwards: the registration stays until some other test's end forgets it, and the registrations stage fails on a second RectoPDF in Open With when none does (CI run 43369527265: `app/build.noindex/RectoPDF.app`, "not a bed's folder; on disk"). Shown by hand: 20 of 20 direct runs of the built binary that ended at once left the build folder registered.
//!
//! Reads the test sources, starts nothing: it is not an app test and is not counted by the ratchet.

use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[path = "common/bed_folder.rs"]
mod bed_folder;

/// The test sources, every file under `tests/`.
fn sources(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// What a `Command::new(...)` on this line is given, if it is the build folder's executable: the built path itself, or a variable named for it.
fn starts_the_build(line: &str) -> Option<String> {
    let code = line.split("//").next().unwrap_or("");
    let mut rest = code;
    while let Some(at) = rest.find("Command::new(") {
        let after = &rest[at + "Command::new(".len()..];
        let argument = after.split(')').next().unwrap_or("").trim().trim_start_matches('&').trim();
        let direct = matches!(argument, "app" | "built") || argument.contains("app_path") || argument.contains("build.noindex");
        if direct {
            return Some(argument.to_owned());
        }
        rest = after;
    }
    None
}

/// Failure: a test runs the built app's executable itself (`Command::new(&app)`), which registers the build folder with Launch Services and leaves it there for the registrations stage to find. A test that needs the real binary runs a copy (`OwnCopy`), which is forgotten, with a look, when it is dropped.
#[test]
fn no_test_starts_the_build_folders_app_itself() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"), &mut files);
    assert!(files.len() > 20, "found no test sources");
    let mut found = Vec::new();
    for file in files {
        if file.file_name().is_some_and(|name| name == "app_launch_guard.rs") {
            continue;
        }
        for (at, line) in std::fs::read_to_string(&file).unwrap().lines().enumerate() {
            if let Some(argument) = starts_the_build(line) {
                found.push(format!("{}:{}: Command::new({argument})", file.display(), at + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these start the build folder's own app; run a copy (`OwnCopy`) instead:\n{}",
        found.join("\n")
    );
}

/// Failure: the check above stops seeing the shapes it is for, and passes on code that starts the app.
#[test]
fn the_check_sees_the_shapes_it_is_for() {
    for line in [
        "let out = Command::new(&app).arg(\"--qa\").output().unwrap();",
        "Command::new(app).spawn()",
        "Command::new(app_path().unwrap())",
        "Command::new(\"../../app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF\")",
        "let a = Command::new(bin()).status(); Command::new(&built)",
    ] {
        assert!(starts_the_build(line).is_some(), "{line}");
    }
    for line in [
        "Command::new(copy.executable()).arg(\"--qa\")",
        "Command::new(bin()).args([\"qa\", \"start\", \"--app\", app.to_str().unwrap()])",
        "// Command::new(&app) is what the guard is about",
        "Command::new(\"/bin/kill\")",
    ] {
        assert!(starts_the_build(line).is_none(), "{line}");
    }
}

/// Failure: quiet-app cleanup signals an unowned numeric PID after exit, risking a signal to an unrelated process that reused it.
#[test]
fn quiet_run_cleanup_never_signals_a_borrowed_pid() {
    let source = include_str!("common/app.rs");
    for (name, code) in [
        (
            "Run::kill",
            source.split("pub fn kill(mut self)").nth(1).expect("Run::kill").split("\n}").next().unwrap(),
        ),
        (
            "Run::drop",
            source.split("impl Drop for Run<'_>").nth(1).expect("Run::drop").split("\n}").next().unwrap(),
        ),
        (
            "fixture end_runs",
            source.split("fn end_runs(").nth(1).expect("fixture end_runs").split("\n}").next().unwrap(),
        ),
        ("fixture lease cleanup", include_str!("common/app/copy_cleanup.rs")),
    ] {
        for forbidden in ["/bin/kill", "signal_process(", "libc::kill("] {
            assert!(!code.contains(forbidden), "{name} signals a borrowed PID through {forbidden}");
        }
    }
}

#[allow(dead_code)]
#[path = "common/app/lifetime.rs"]
mod lifetime;
#[allow(dead_code)]
#[path = "../src/qa_launch.rs"]
mod qa_launch;

#[cfg(unix)]
fn waiting_child() -> std::process::Child {
    use std::io::BufRead;
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "printf 'ready\n'; exec /bin/cat"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    child
}

/// Failure: hard cleanup is attempted on an attached process, or an owned killed child is left unreaped and its PID unanchored.
#[cfg(unix)]
#[test]
fn owned_quiet_children_are_reaped_and_borrowed_instances_cannot_be_killed() {
    eprintln!(
        "test harness compiler sizes: Child={}, Lifetime={}",
        std::mem::size_of::<std::process::Child>(),
        std::mem::size_of::<lifetime::Lifetime>()
    );
    let mut borrowed = lifetime::Lifetime::Borrowed(None);
    assert!(borrowed.terminate_owned().unwrap_err().contains("requires"));
    assert!(borrowed.kill_owned(std::time::Duration::ZERO).unwrap_err().contains("requires"));
    let child = waiting_child();
    let pid = child.id();
    let mut owned = lifetime::Lifetime::Owned(child);
    owned.kill_owned(std::time::Duration::from_secs(1)).unwrap();
    assert!(owned.gone(pid).unwrap(), "the Child is reaped, not just absent from a PID lookup");
    owned.kill_owned(std::time::Duration::ZERO).unwrap();
}

/// Failure: a rejected readiness line drops a still-live owned child, or replaces the real readiness failure with teardown noise.
#[cfg(unix)]
#[test]
fn rejected_launches_preserve_their_error_and_reap_the_owned_child() {
    let mut owner = lifetime::Lifetime::Owned(waiting_child());
    let lifetime::Lifetime::Owned(child) = &mut owner else { unreachable!() };
    let original = recto_agent::AgentError::new(recto_agent::Code::APP_REFUSED, "fixture malformed readiness", 1);
    let returned = qa_launch::cleanup_error(child, original, std::time::Duration::from_secs(1));
    assert_eq!(returned.code, "app_refused");
    assert_eq!(returned.message, "fixture malformed readiness");
    assert!(child.try_wait().unwrap().is_some(), "failed launch left its owned child alive or unreaped");
}

/// Failure: a ready line names another process and ownership is lost before the caller can refuse it. Valid readiness transfers ownership without killing.
#[cfg(unix)]
#[test]
fn readiness_names_only_the_child_that_was_actually_started() {
    let mut owner = lifetime::Lifetime::Owned(waiting_child());
    let lifetime::Lifetime::Owned(child) = &mut owner else { unreachable!() };
    let wait = std::time::Duration::from_secs(1);
    let ready = qa_launch::ready_value(child, serde_json::json!({"ok": true, "data": {"pid": child.id()}}), wait).unwrap();
    assert_eq!(ready["pid"].as_u64(), Some(u64::from(child.id())));
    assert!(child.try_wait().unwrap().is_none(), "accepting readiness must not terminate the child");
    let refused = qa_launch::ready_value(child, serde_json::json!({"ok": true, "data": {"pid": 1}}), wait).unwrap_err();
    assert_eq!(refused.code, "app_refused");
    assert!(refused.message.contains("identify"));
    assert!(child.try_wait().unwrap().is_some(), "a lying ready line still gets owned-child cleanup");
}

/// Failure: an executable grows the launcher's ready-line buffer without bound, or the bound discards a valid line at the limit.
#[test]
fn an_external_ready_line_is_capped_before_json_decoding() {
    let limit = 16;
    let valid = format!("{}\n", "x".repeat(limit - 1));
    assert_eq!(qa_launch::read_ready_line(std::io::Cursor::new(&valid), limit).unwrap(), valid);
    let too_long = format!("{}\n", "x".repeat(limit));
    let error = qa_launch::read_ready_line(std::io::Cursor::new(&too_long), limit).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exceeds"));
    assert!(qa_launch::read_ready_line(std::io::Cursor::new([255u8]), limit).is_err());
}

/// Failure: empty startup stdout makes the launcher read an unbounded stderr file, or drops the useful prefix without saying it was truncated.
#[cfg(unix)]
#[test]
fn failed_startup_stderr_is_a_bounded_explicit_excerpt() {
    let folder = recto_testpdf::TempDir::new("ready-stderr-");
    let file = folder.join("messages");
    drop(recto_platform::local::create_private_file(&file).unwrap());
    std::fs::write(&file, "small failure").unwrap();
    assert_eq!(qa_launch::stderr_excerpt(&file, 16), "small failure");
    std::fs::write(&file, format!("{}SECRET_TAIL", "p".repeat(16))).unwrap();
    let excerpt = qa_launch::stderr_excerpt(&file, 16);
    assert!(excerpt.starts_with(&"p".repeat(16)), "the failure prefix survives");
    assert!(excerpt.contains("truncated") && !excerpt.contains("SECRET_TAIL"), "{excerpt}");
    assert!(excerpt.len() <= 16 + 64, "only the bounded excerpt and fixed truncation notice are kept");
}

#[allow(dead_code)]
#[path = "common/app/copy_cleanup.rs"]
mod copy_cleanup;
#[allow(dead_code)]
#[path = "../../recto-app/src/core/qa/program_lease.rs"]
mod program_lease;

/// Failure: deleting a just-started quiet copy makes current_exe fail, leaving no program cancellation lease; relative argv0 must never watch an unrelated file.
#[test]
fn quiet_program_fallback_is_an_absolute_cancellation_lease_only() {
    let current = PathBuf::from("/real/program");
    let fallback = std::ffi::OsString::from("/deleted/program");
    assert_eq!(program_lease::program(Some(current.clone()), Some(fallback.clone())), Some(current));
    assert_eq!(program_lease::program(None, Some(fallback)), Some(PathBuf::from("/deleted/program")));
    assert_eq!(program_lease::program(None, Some("relative/program".into())), None);
    assert_eq!(program_lease::program(None, None), None);
    let source = include_str!("../../recto-app/src/core/qa/mod.rs");
    let guard = source
        .split("pub fn end_with_starter")
        .nth(1)
        .unwrap()
        .split("fn updates_takes_input")
        .next()
        .unwrap();
    assert!(guard.contains("leave_now(124)"), "C exit handlers must not strand the quiet lifetime guard");
}

/// Failure: fixture lease cancellation renames the live inode, loses bytes/mode, or overwrites a replacement when restoring its path.
#[cfg(unix)]
#[test]
fn fixture_program_lease_restores_bytes_and_never_overwrites_replacement() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let folder = recto_testpdf::TempDir::new("lease-backup-");
    let fixture = folder.join(format!("own-{}-1", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let program = fixture.join("RectoPDF");
    std::fs::write(&program, "original bytes").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let as_alive = |_pid: u32| Some((1, 1));
    let refused = copy_cleanup::cancel_by(&fixture, &program, &[std::process::id()], std::time::Duration::ZERO, as_alive).unwrap_err();
    assert!(refused.contains("no PID was signalled") && refused.contains("did not exit"), "{refused}");
    assert_eq!(
        std::fs::read(&program).unwrap(),
        b"original bytes",
        "timeout restores the program before reporting failure"
    );
    let inode = std::fs::metadata(&program).unwrap().ino();
    let lease = copy_cleanup::Lease::remove(&fixture, &program).unwrap();
    assert!(!program.exists());
    lease.restore().unwrap();
    assert_eq!(std::fs::read(&program).unwrap(), b"original bytes");
    assert_eq!(std::fs::metadata(&program).unwrap().permissions().mode() & 0o777, 0o700);
    assert_ne!(std::fs::metadata(&program).unwrap().ino(), inode, "backup must have a distinct inode");
    let lease = copy_cleanup::Lease::remove(&fixture, &program).unwrap();
    std::fs::write(&program, "replacement").unwrap();
    assert!(lease.restore().unwrap_err().contains("backup retained"));
    assert_eq!(std::fs::read(&program).unwrap(), b"replacement");
    assert!(
        copy_cleanup::Lease::remove(&folder, &program).is_err(),
        "an arbitrary root is never mutation authority"
    );
}

/// Failure: explicit quit/handoff reconnects to replacement state instead of the attached exact instance.
#[test]
fn explicit_lifecycle_commands_use_the_retained_instance_connection() {
    let source = include_str!("common/app.rs");
    for name in ["pub fn quit(mut self)", "pub fn hand_off(mut self)"] {
        let code = source.split(name).nth(1).unwrap().split("\n    }").next().unwrap();
        assert!(code.contains(".lifetime"));
        assert!(!code.contains("state.json"));
    }
}

/// The guard binary doubles as a tiny native vnode observer; no app, timer thread or polling workload is started.
#[cfg(unix)]
#[test]
fn fixture_lease_watcher_child() {
    use std::io::Write;
    let Some(program) = std::env::var_os("RECTO_TEST_PROGRAM_LEASE") else {
        return;
    };
    let program = PathBuf::from(program);
    println!("LEASE_READY");
    std::io::stdout().flush().unwrap();
    if std::env::var_os("RECTO_TEST_ARM_AFTER_LINE").is_some() {
        // A run still starting when its program is taken away: it arms its watch only when told, as the app does at the end of its start.
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }
    let gone = recto_platform::local::wait_until_gone(&[], &[&program], std::time::Duration::from_secs(5));
    assert!(matches!(gone, recto_platform::local::Gone::Path(_)));
}

/// Failure: removing the program lease fails to wake the existing native quiet watcher, or restoring it loses executable bytes.
#[cfg(unix)]
#[test]
fn a_removed_fixture_lease_ends_its_observer_then_restores() {
    use std::io::BufRead;
    let folder = recto_testpdf::TempDir::new("lease-observer-");
    let fixture = folder.join(format!("own-{}-2", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let program = fixture.join("RectoPDF");
    std::fs::write(&program, "original").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fixture_lease_watcher_child", "--nocapture"])
        .env("RECTO_TEST_PROGRAM_LEASE", &program)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut owner = lifetime::Lifetime::Owned(child);
    let mut line = String::new();
    loop {
        assert!(output.read_line(&mut line).unwrap() > 0, "observer exited before readiness: {line}");
        if line.contains("LEASE_READY") {
            break;
        }
        line.clear();
    }
    let lease = copy_cleanup::Lease::remove(&fixture, &program).unwrap();
    let lifetime::Lifetime::Owned(child) = &mut owner else { unreachable!() };
    assert!(child.wait().unwrap().success(), "native watcher exits by its removed lease, with no signal");
    lease.restore().unwrap();
    assert_eq!(std::fs::read(&program).unwrap(), b"original");
}

#[allow(dead_code)]
#[path = "../../recto-platform/src/observed_program.rs"]
mod observed_program;

/// Failure: deleting a fixture executable makes fresh process scans return empty before inspecting any PID; only an absolute missing basename with a canonical existing parent is safe for observation.
#[test]
fn missing_program_observation_keeps_only_a_canonical_parent_and_basename() {
    use std::io::{Error, ErrorKind};
    let missing = || Err(Error::from(ErrorKind::NotFound));
    let program = Path::new("/aliased/RectoPDF");
    let selected = observed_program::select(program, missing(), |parent| {
        assert_eq!(parent, Path::new("/aliased"));
        Ok(PathBuf::from("/canonical"))
    })
    .unwrap();
    assert_eq!(selected, Path::new("/canonical/RectoPDF"));
    assert!(observed_program::select(Path::new("relative/RectoPDF"), missing(), |_| panic!("relative paths never fall back")).is_err());
    assert!(
        observed_program::select(program, missing(), |_| missing()).is_err(),
        "a missing parent never falls back"
    );
    assert!(
        observed_program::select(program, Err(Error::from(ErrorKind::PermissionDenied)), |_| panic!(
            "permission failures never fall back"
        ))
        .is_err()
    );
    assert_eq!(
        observed_program::select(program, Ok(PathBuf::from("/existing")), |_| panic!("existing canonical paths stay unchanged")).unwrap(),
        Path::new("/existing")
    );
    let source = include_str!("../../recto-platform/src/local_mac.rs");
    let scan = source
        .split("pub fn processes_running(")
        .nth(1)
        .unwrap()
        .split("/// The path a process")
        .next()
        .unwrap();
    assert!(scan.contains("observed_program::path(executable)"));
}

/// Failure: the SIGTERM lifecycle case attaches to a CLI-launched process, so its signal lacks retained Child ownership.
#[test]
fn the_sigterm_lifecycle_case_launches_and_retains_its_child() {
    let update = include_str!("restore/update.rs");
    let case = update
        .split("fn a_quit_isnt_an_early_end_and_a_kill_is()")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert!(
        case.contains("bed.start_owned()"),
        "SIGTERM must start an owned Child rather than attach to a CLI launch"
    );
    let owned = update.split("fn start_owned(").nth(1).unwrap().split("\n    }").next().unwrap();
    assert!(owned.contains("Run::on_executable_with_env") && owned.contains("REPO_ENV") && owned.contains("self.server.url"));
    let app = include_str!("common/app.rs");
    let launch = app.split("pub fn on_executable_with_env(").nth(1).unwrap().split("\n    }").next().unwrap();
    assert!(launch.contains("launch(state,") && launch.contains("Lifetime::Owned(child)"));
}

/// Failure: missing-program observation widens the shipping updater's process selection before it sends a signal.
#[test]
fn missing_program_observation_cannot_widen_shipping_signal_selection() {
    let source = include_str!("../../recto-platform/src/local_mac.rs");
    let standard = source.split("pub fn processes_running(").nth(1).unwrap().split("\n}").next().unwrap();
    assert!(
        standard.contains("executable.canonicalize()"),
        "the standard resolver must require an existing canonical executable"
    );
    assert!(!standard.contains("observed_program"));
    assert!(source.contains("#[cfg(feature = \"test-seams\")]\npub fn observe_missing_program_processes"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources(&root.join("crates"), &mut files);
    let calls: Vec<_> = files
        .into_iter()
        .filter(|path| {
            path.file_name().is_none_or(|name| name != "app_launch_guard.rs")
                && std::fs::read_to_string(path).unwrap().contains("local::observe_missing_program_processes(")
        })
        .collect();
    assert_eq!(calls, [root.join("crates/recto-cli/tests/common/app/copy_cleanup.rs")]);
}

/// Failure: fixture cleanup reports only an OS error, leaving native failures impossible to assign to source-open, backup, copy or unlink.
#[cfg(unix)]
#[test]
fn fixture_cleanup_errors_name_the_failing_operation() {
    use std::os::unix::fs::PermissionsExt;
    let folder = recto_testpdf::TempDir::new("lease-error-context-");
    let fixture = folder.join(format!("own-{}-3", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let program = fixture.join("RectoPDF");
    std::fs::write(&program, "unreadable fixture").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o0)).unwrap();
    let error = copy_cleanup::Lease::remove(&fixture, &program)
        .err()
        .expect("made source-open permission failure");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        error.starts_with("open fixture program ") && error.contains(&program.display().to_string()),
        "{error}"
    );
}

/// Failure: cleanup creates a backup resource inside an active app bundle, where native fixture creation has been denied before unlinking can cancel it.
#[cfg(unix)]
#[test]
fn fixture_program_backup_stays_outside_the_app_bundle() {
    let folder = recto_testpdf::TempDir::new("lease-outside-bundle-");
    let fixture = folder.join(format!("own-{}-4", std::process::id()));
    let macos = fixture.join("RectoPDF.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let macos = macos.canonicalize().unwrap();
    let program = macos.join("RectoPDF");
    std::fs::write(&program, "fixture program").unwrap();
    let lease = copy_cleanup::Lease::remove(&fixture.join("RectoPDF.app"), &program).unwrap();
    assert_eq!(std::fs::read_dir(&macos).unwrap().count(), 0, "the app bundle receives no new backup resource");
    let backups: Vec<_> = std::fs::read_dir(&fixture)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".recto-cleanup-"))
        .collect();
    assert_eq!(backups.len(), 1, "the validated owned fixture root holds the live backup");
    assert_eq!(std::fs::read(backups[0].path()).unwrap(), b"fixture program");
    lease.restore().unwrap();
    assert_eq!(std::fs::read(&program).unwrap(), b"fixture program");
}

/// Failure: a long workload attaches to its own CLI launch, loses its idle borrowed connection before Quit, or accidentally inherits the short three-minute lifetime.
#[test]
fn long_session_keeps_its_owned_child_and_thirty_minute_limit() {
    let source = include_str!("long_session.rs");
    let start = source
        .split("fn a_long_session_stays_flat()")
        .nth(1)
        .unwrap()
        .split("let workload =")
        .next()
        .unwrap();
    assert!(
        start.contains("Run::on_executable_with_env"),
        "the long session must retain the original launch Child"
    );
    assert!(
        start.contains("Duration::from_secs(30 * 60)"),
        "the long workload must keep its existing thirty-minute lifetime"
    );
    assert!(
        !start.contains("Run::attach") && !start.contains("state.json("),
        "the long workload cannot detach and borrow its own launch"
    );
    let app = include_str!("common/app.rs");
    let launch = app.split("pub fn on_executable_with_env(").nth(1).unwrap().split("\n    }").next().unwrap();
    assert!(
        launch.contains("limit.as_secs()") && launch.contains("\"--time-limit\", &limit"),
        "the supplied lifetime must reach the shared CLI launcher"
    );
    assert!(launch.contains("Lifetime::Owned(child)"));
}

/// Failure: cleanup's wait for a copy's exit is a fixed 5 s, not the bound a quiet run has to arm its watcher (`READY_CAP` plus the time to exit), and the
/// watcher is armed after the ready line, so a run is up (and the start's cap spent) before it can notice its program go (CI: `a_copy_opened_and_dropped_at_once_is_forgotten`,
/// 22.4 s under load 23, the app did exit).
#[test]
fn cleanups_wait_for_a_copys_exit_is_the_bound_a_quiet_run_arms_its_watch_in() {
    assert!(
        copy_cleanup::EXIT_CAP >= qa_launch::READY_CAP + copy_cleanup::EXIT_GRACE,
        "EXIT_CAP {:?} is shorter than READY_CAP {:?} plus the grace to exit",
        copy_cleanup::EXIT_CAP,
        qa_launch::READY_CAP
    );
    let source = include_str!("common/app.rs");
    let end_runs = source.split("fn end_runs(").nth(1).expect("end_runs").split("\n}").next().unwrap();
    assert!(
        end_runs.contains("copy_cleanup::EXIT_CAP"),
        "end_runs gives cleanup a wait of its own, not EXIT_CAP"
    );
    // The watch is armed by the end of the app's start, and the ready line is written by `ready`, which the host calls after `start` returned.
    let start = include_str!("../../recto-app/src/core/start.rs");
    let arming = start.split("fn start_with(").nth(1).expect("start_with").split("\n    }\n").next().unwrap();
    assert!(arming.contains("qa::end_with_starter("), "start_with no longer arms the quiet run's end-watch");
    assert!(
        !start.contains("\"running\": true"),
        "the ready line is written inside start, before the watch is armed"
    );
    let ffi = include_str!("../../recto-ffi/src/lib.rs");
    let started = ffi.find("let inner = Inner::start(").expect("the bridge's start");
    let ready_line = ffi.find("\"running\": true").expect("the ready line");
    assert!(started < ready_line, "the ready line comes before the app's start has returned");
    assert!(
        ffi[started..ready_line].contains("pub fn ready("),
        "the ready line is not in `ready`, which follows `start`"
    );
}

/// Failure: cleanup gives a copy its program back at the first slow moment, so a run that arms its watch after that finds its program present, never sees it
/// go, and lives on (the harness bug behind the 5 s cap: `cancel` restored the program as a new inode when its time ran out). The run here arms only after
/// the program is absent, as a copy still starting does. Also the failures it names: a copy dropped at the identity check (cleanup says Ok while the copy
/// still runs) and a cleanup that never looks at the copy. It holds the wait on the exit by `kevent` (no polling), that no signal is sent, and the
/// byte-for-byte restore. Not the cap: that is held by `cleanups_wait_for_a_copys_exit_is_the_bound_a_quiet_run_arms_its_watch_in`.
#[cfg(unix)]
#[test]
fn a_run_that_arms_its_watch_late_still_ends_by_the_lease_and_the_program_is_restored() {
    use std::io::{BufRead, Write};
    use std::os::unix::process::ExitStatusExt;
    let folder = recto_testpdf::TempDir::new("lease-late-");
    let fixture = folder.join(format!("own-{}-5", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let program = fixture.join("RectoPDF");
    // A copy, not a clone: the temp folder may be on another volume than the build.
    std::fs::copy(std::env::current_exe().unwrap(), &program).unwrap();
    let mut child = std::process::Command::new(&program)
        .args(["--exact", "fixture_lease_watcher_child", "--nocapture"])
        .env("RECTO_TEST_PROGRAM_LEASE", &program)
        .env("RECTO_TEST_ARM_AFTER_LINE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let child_pid = child.id();
    let mut gate = child.stdin.take().unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut owner = lifetime::Lifetime::Owned(child);
    let mut line = String::new();
    loop {
        assert!(output.read_line(&mut line).unwrap() > 0, "child exited before readiness: {line}");
        if line.contains("LEASE_READY") {
            break;
        }
        line.clear();
    }
    // The gate opens only once cleanup has taken the lease and looked at the copy twice (when it was taken in, and the check that it is still the copy): a
    // copy dropped at that look would let cleanup say Ok with the copy still running, and a gate opened at once would hide that.
    let (looked, looks) = std::sync::mpsc::channel();
    let cleaning = {
        let (fixture, program) = (fixture.clone(), program.clone());
        std::thread::spawn(move || {
            let started = move |pid| {
                let at = copy_cleanup::start_time(pid);
                let _ = looked.send(());
                at
            };
            let ended = copy_cleanup::cancel_by(&fixture, &program, &[child_pid], copy_cleanup::EXIT_CAP, started);
            // Whether the copy was still running when cleanup said it was done (a zombie, exited and not yet reaped, has no counters).
            (ended, recto_platform::local::process_counters(child_pid).is_some())
        })
    };
    for _ in 0..2 {
        looks
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("cleanup never looked at the copy");
    }
    // The program's removal is a NOTE_DELETE, seen however short the absence: no polling.
    let gone = recto_platform::local::wait_until_gone(&[], &[&program], std::time::Duration::from_secs(10));
    assert!(
        matches!(gone, recto_platform::local::Gone::Path(_)),
        "cleanup never took the program away: {gone:?}"
    );
    writeln!(gate, "arm").unwrap();
    gate.flush().unwrap();
    let (ended, still_running) = cleaning.join().unwrap();
    let lifetime::Lifetime::Owned(child) = &mut owner else { unreachable!() };
    let status = child.wait().unwrap();
    assert!(ended.is_ok(), "{ended:?}");
    assert!(!still_running, "cleanup returned Ok while the copy was still running");
    assert!(
        status.success() && status.signal().is_none(),
        "the run ends by its lease, not a signal: {status:?}"
    );
    assert_eq!(
        std::fs::read(&program).unwrap(),
        std::fs::read(std::env::current_exe().unwrap()).unwrap(),
        "the program is back, byte for byte"
    );
    assert!(
        std::fs::read_dir(&fixture)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".recto-cleanup-")),
        "no backup is left"
    );
}

/// Failure: a pid that was observed as the copy and has been reused by an unrelated process is waited for as the copy: the cleanup runs to its cap and fails.
#[cfg(unix)]
#[test]
fn a_reused_pid_is_not_waited_for_as_the_copy() {
    let folder = recto_testpdf::TempDir::new("lease-reuse-");
    let fixture = folder.join(format!("own-{}-6", std::process::id()));
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let program = fixture.join("RectoPDF");
    std::fs::write(&program, "original").unwrap();
    // This test's own pid was "observed"; its start time differs at the second look, as a number given to another process after the copy exited does.
    let looks = std::cell::Cell::new(0);
    let reused = |_pid: u32| {
        looks.set(looks.get() + 1);
        Some((looks.get(), 0))
    };
    copy_cleanup::cancel_by(&fixture, &program, &[std::process::id()], std::time::Duration::from_secs(30), reused).unwrap();
    assert_eq!(std::fs::read(&program).unwrap(), b"original");
}

/// Failure: the start's wait is wall clock again, so a start that is slow but working (CPU or page-ins still rising) is stopped as a hung one is (CI lifecycle, 5 of 30 runs: "wasn't ready in time"),
/// or one that has stopped is waited for to the cap, or the cap is not kept.
#[test]
fn a_start_is_waited_for_while_it_makes_progress_and_stopped_when_it_does_not() {
    use qa_launch::{Patience, Verdict, ready_verdict};
    use std::time::Duration;
    let patience = Patience {
        stall: Duration::from_secs(10),
        cap: Duration::from_secs(180),
    };
    let at = |elapsed: u64, quiet: u64| ready_verdict(Duration::from_secs(elapsed), Duration::from_secs(quiet), patience);
    assert_eq!(at(100, 0), Verdict::Wait, "far past the old 30 s, and still working");
    assert_eq!(at(100, 9), Verdict::Wait);
    assert_eq!(at(100, 10), Verdict::Stalled);
    assert_eq!(at(11, 10), Verdict::Stalled, "a hang is stopped at the stall, not the cap");
    assert_eq!(at(180, 0), Verdict::Capped, "working to the end is still ended at the cap");
    assert_eq!(at(180, 50), Verdict::Capped);
    assert!(qa_launch::READY_CAP > std::time::Duration::from_secs(30) && qa_launch::READY_STALL < qa_launch::READY_CAP);
}

/// Failure: a quiet run's own limit is shorter than the harness's, so the app ends mid-test while the test is still allowed to run (2026-11-26, a closed-window test).
#[test]
fn a_quiet_runs_limit_is_the_harness_limit_on_a_test() {
    let nextest = include_str!("../../../.config/nextest.toml");
    assert!(
        nextest.contains("slow-timeout = { period = \"60s\", terminate-after = 5 }"),
        "nextest's limit changed: TEST_LIMIT (tests/common/app.rs) follows it"
    );
    let app = include_str!("common/app.rs");
    assert!(app.contains("pub const TEST_LIMIT: Duration = Duration::from_secs(300);"));
    assert!(!app.contains("Duration::from_secs(120), extra") && !app.contains("Duration::from_secs(3 * 60), env"));
}

/// A shell script standing in for the app: it takes the app's arguments and ignores them.
#[cfg(target_os = "macos")]
fn stand_in(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("stand-in");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(target_os = "macos")]
fn started(body: &str) -> (std::time::Duration, recto_agent::AgentError) {
    let dir = std::env::temp_dir().join(format!("recto-ready-patience-{}-{}", std::process::id(), body.len()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = stand_in(&dir, body);
    let paths = recto_control::StatePaths::overridden(&dir.join("state")).expect("a state folder in the temp folder");
    let options = qa_launch::Launch {
        patience: qa_launch::Patience {
            stall: std::time::Duration::from_millis(300),
            cap: std::time::Duration::from_millis(900),
        },
        ..Default::default()
    };
    let began = std::time::Instant::now();
    let error = qa_launch::launch(&app, &paths, &options, &[]).expect_err("a stand-in that never says ready is stopped");
    let took = began.elapsed();
    let _ = std::fs::remove_dir_all(&dir);
    (took, error)
}

/// Failure (the verdict, wired to the real counters): a start doing nothing runs to the cap as wall clock would, or one burning CPU is stopped at the stall.
/// Stand-ins, not the app: about 1.5 s together.
#[cfg(target_os = "macos")]
#[test]
fn a_start_that_does_nothing_is_stopped_at_the_stall_and_one_that_works_runs_to_the_cap() {
    let (idle, error) = started("exec sleep 30");
    assert!(error.message.contains("made no progress"), "{}", error.message);
    assert!(
        idle < std::time::Duration::from_millis(800),
        "stopped at the stall (300 ms), not the cap (900 ms): {idle:?}"
    );
    let (busy, error) = started("while :; do :; done");
    assert!(error.message.contains("still starting"), "{}", error.message);
    assert!(
        busy >= std::time::Duration::from_millis(900),
        "a start that is working is waited for to the cap: {busy:?}"
    );
    assert!(!error.next.contains("--timeout"), "{}", error.next);
}
