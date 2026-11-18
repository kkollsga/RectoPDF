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
    let refused = copy_cleanup::cancel(&fixture, &program, &[std::process::id()], std::time::Duration::ZERO).unwrap_err();
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
