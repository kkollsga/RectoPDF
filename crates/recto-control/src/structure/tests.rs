use super::*;
use recto_platform::helper::Priority;
use std::sync::atomic::AtomicBool;

/// Lines in, kept and counted; `end` ends it.
#[derive(Debug, Default)]
struct Lines(Vec<String>);

impl Answer for Lines {
    fn take(&mut self, line: &[u8]) -> Result<Said, &'static str> {
        let line = String::from_utf8_lossy(line).into_owned();
        let said = if line == "end" { Said::End } else { Said::More };
        self.0.push(line);
        Ok(said)
    }
}

/// A folder of its own for stand-in helpers and what they leave, removed at the end.
fn folder(name: &str) -> recto_testpdf::TempDir {
    recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), &format!("recto-structure-{name}-"))
}

/// A stand-in helper: a shell script, started as the real one is.
fn stand_in(folder: &Path, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = folder.join("stand-in");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn never() -> (Instant, AtomicBool) {
    (Instant::now() + Duration::from_secs(60), AtomicBool::new(false))
}

/// Whether process `pid` is still there, by the signal that touches nothing.
fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Failure: the time a job gets doesn't grow with the document, so a long one is always killed, or has no ceiling.
#[test]
fn the_time_allowed_grows_with_pages_and_has_a_ceiling() {
    assert!(time_limit(383) > time_limit(10));
    assert_eq!(time_limit(u32::MAX), Duration::from_secs(30 * 60));
}

/// Failure: the outside check lets a helper through on a word of its own (it says it's walled and the kernel disagrees), on a kernel that couldn't say (-1), or when the document is
/// readable by one of its two names, so a compromised helper is given a document. Only all three together pass.
#[test]
fn only_a_helper_the_kernel_shows_walled_is_admitted() {
    assert_eq!(admit(Some(true), 1, &[1, 1]), Ok(()));
    for (walled, sandboxed, by_name) in [
        (Some(false), 1, [1, 1]),
        (None, 1, [1, 1]),
        (Some(true), 0, [1, 1]),
        (Some(true), -1, [1, 1]),
        (Some(true), 1, [0, 1]),
        (Some(true), 1, [1, 0]),
        (Some(true), 1, [1, -1]),
    ] {
        assert_eq!(
            admit(walled, sandboxed, &by_name),
            Err(WallsDown::NotSandboxed),
            "{walled:?} {sandboxed} {by_name:?}"
        );
    }
    assert_eq!(admit(Some(true), 1, &[]), Ok(()), "nothing asked about a name is nothing to refuse");
}

/// Failure: a helper that ended before saying hello is told by the wrong word: a wall that wouldn't go up is "stopped", or a kill on starting inside another sandbox is called a wrong signature.
#[test]
fn how_a_helper_ended_before_its_hello_says_which_wall() {
    assert_eq!(why_it_ended(Some(77), false), "structure_not_sandboxed");
    assert_eq!(why_it_ended(Some(78), false), "structure_walls_failed");
    assert_eq!(why_it_ended(Some(79), false), "structure_walls_failed");
    assert_eq!(why_it_ended(None, true), "structure_inside_another_sandbox");
    assert_eq!(why_it_ended(None, false), "helper_stopped", "killed, and nothing else in the way");
    assert_eq!(why_it_ended(Some(0), true), "helper_stopped");
    assert_eq!(why_it_ended(Some(3), false), "helper_stopped");
}

/// Failure: a code is spelled one way here and another in the table, so an agent is told "ask again in a minute" for a copy that is signed wrong.
#[test]
fn each_way_the_walls_can_be_down_has_its_own_code() {
    let all = [
        WallsDown::NotSandboxed,
        WallsDown::InsideAnotherSandbox,
        WallsDown::WallsFailed,
        WallsDown::StartRefused,
    ];
    let codes: std::collections::HashSet<_> = all.iter().map(|down| down.code()).collect();
    assert_eq!(codes.len(), 4);
    assert!(codes.iter().all(|code| code.starts_with("structure_")));
}

/// Failure: a line past the cap is held whole before it's refused (a hostile helper's memory is ours), or one over the total, or an answer that stops mid-line is believed. At the cap, it's read.
#[test]
fn a_line_is_read_up_to_its_cap_and_the_total_has_a_ceiling() {
    let mut budget = MAX_ANSWER_BYTES;
    let mut buf = Vec::new();
    let at_the_cap = [vec![b'a'; MAX_LINE_BYTES], vec![b'\n']].concat();
    assert_eq!(next_line(&mut &at_the_cap[..], &mut buf, &mut budget), Ok(true));
    assert_eq!(buf.len(), MAX_LINE_BYTES);
    let over = [vec![b'a'; MAX_LINE_BYTES + 1], vec![b'\n']].concat();
    assert_eq!(next_line(&mut &over[..], &mut buf, &mut budget), Err("bad_answer"));
    assert!(buf.len() <= MAX_LINE_BYTES, "nothing past the cap is held: {}", buf.len());
    // Past the total: a budget of 10 takes a line of 9 and its newline, and not one of 10.
    let (mut tight, ok, long) = (10u64, b"123456789\nrest", b"1234567890\n");
    assert_eq!(next_line(&mut &ok[..], &mut buf, &mut tight), Ok(true));
    assert_eq!(tight, 0);
    let mut tight = 10u64;
    assert_eq!(next_line(&mut &long[..], &mut buf, &mut tight), Err("bad_answer"));
    let mut whole = MAX_ANSWER_BYTES;
    assert_eq!(next_line(&mut &b"cut off"[..], &mut buf, &mut whole), Err("bad_answer"));
    assert_eq!(next_line(&mut &b""[..], &mut buf, &mut whole), Ok(false));
}

/// Failure: a 64 KiB line of `[` overflows the reader thread's stack in a debug build (serde_json's recursion needs more than 64 KiB there) and takes the whole app with it, instead of being
/// refused as a bad line.
#[test]
fn a_line_of_open_brackets_is_refused_on_the_readers_own_stack() {
    let read = std::thread::Builder::new()
        .stack_size(READER_STACK)
        .spawn(|| {
            let line = vec![b'['; MAX_LINE_BYTES];
            let mut answer = headings::HeadingsAnswer::new(10, 100);
            Answer::take(&mut answer, &line)
        })
        .unwrap()
        .join();
    assert_eq!(read.expect("no overflow"), Err("bad_answer"));
}

/// Failure: a helper that sends a line past the cap is believed or waited out. With a stand-in that says hello and then 70,000 bytes before it would sleep for ten minutes: the answer is
/// `bad_answer` and the process is gone, within 5 s.
#[test]
fn a_line_over_the_cap_ends_the_helper_at_once() {
    let folder = folder("over");
    let pid_file = folder.join("pid");
    let exe = stand_in(
        &folder,
        &format!(
            "echo $$ > '{}'\necho '{{\"state\":\"hello\",\"walled\":true}}'\nhead -c 70000 /dev/zero | tr '\\0' a\necho\nexec /bin/sleep 600",
            pid_file.display()
        ),
    );
    let spawned = helper::spawn(&exe, &[], Priority::Inherited).unwrap();
    let (deadline, stop) = never();
    let started = Instant::now();
    let (ended, numbers) = talk(spawned, &json!({ "kind": "x" }), Lines::default(), deadline, &stop, |_, _| Ok(()));
    assert!(matches!(&ended, Ended::Failed(code) if code == "bad_answer"), "{ended:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(numbers["walled"], json!(true), "admitted before it said it");
    let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(!alive(pid), "the helper is gone");
}

/// Failure: a helper that says it is walled and isn't (here a script that says so, and writes whatever it is given to a file) gets its job anyway, so the first thing an unwalled one does is
/// read one. The real outside check finds no sandbox on it, it's killed, and what it would have written isn't there.
#[test]
fn a_helper_that_lies_about_its_walls_is_never_given_the_job() {
    let folder = folder("liar");
    let (mark, document) = (folder.join("mark"), folder.join("doc.pdf"));
    std::fs::write(&document, b"%PDF-1.7").unwrap();
    let exe = stand_in(
        &folder,
        &format!("echo '{{\"state\":\"hello\",\"walled\":true}}'\nread job\necho \"$job\" > '{}'", mark.display()),
    );
    let spawned = helper::spawn(&exe, &[], Priority::Inherited).unwrap();
    let (deadline, stop) = never();
    // The real check, after a moment in which a job already written would have been read by the stand-in and written out.
    let check = |pid, walled| {
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until && !mark.exists() {
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        outside_check(pid, walled, &document)
    };
    let (ended, _) = talk(spawned, &json!({ "kind": "headings" }), Lines::default(), deadline, &stop, check);
    assert!(matches!(&ended, Ended::Failed(code) if code == "structure_not_sandboxed"), "{ended:?}");
    assert!(!mark.exists(), "it was given its job: {:?}", std::fs::read_to_string(&mark));
}

/// Failure: the helper is started with more than `MallocLargeCache=0` (the render worker's and PDFium's locations the indexer's helpers take, a sandbox's container id, this process's whole
/// environment), or without it, which put 281 MiB on the NASA report where 39 is.
#[test]
fn the_helper_is_started_with_one_variable_and_no_other() {
    let folder = folder("env");
    let document = folder.join("doc.pdf");
    std::fs::write(&document, b"%PDF-1.7").unwrap();
    let file = std::fs::File::open(&document).unwrap();
    // The environment of a process that prints it is the one thing it was given.
    let Spawned { mut child, stdin, mut stdout } = spawn(Path::new("/usr/bin/env"), Priority::Inherited, &file).unwrap();
    let mut said = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut said).unwrap();
    drop(stdin);
    let _ = child.wait();
    assert_eq!(said, "MallocLargeCache=0\n");
}

/// An answer that ends at once and reports numbers that look like the session's own.
#[derive(Debug, Default)]
struct Boasts;

impl Answer for Boasts {
    fn take(&mut self, _line: &[u8]) -> Result<Said, &'static str> {
        Ok(Said::End)
    }
    fn numbers(&self) -> Value {
        json!({ "walled": false, "elapsed_ms": 999_999_999u64, "helper_peak_watched_bytes": 1, "kept": 7 })
    }
}

/// Failure: the numbers an answer reports are merged over the session's own, so a helper that gets one through can say it wasn't walled, or that it took no time. The session's win, and the
/// answer's other numbers stay.
#[test]
fn the_sessions_numbers_win_over_the_answers() {
    let folder = folder("numbers");
    let exe = stand_in(&folder, "echo '{\"state\":\"hello\",\"walled\":true}'\necho end");
    let spawned = helper::spawn(&exe, &[], Priority::Inherited).unwrap();
    let (deadline, stop) = never();
    let (ended, numbers) = talk(spawned, &json!({ "kind": "x" }), Boasts, deadline, &stop, |_, _| Ok(()));
    assert!(matches!(ended, Ended::Answered(_)), "{ended:?}");
    assert_eq!(numbers["walled"], json!(true));
    assert!(numbers["elapsed_ms"].as_u64().unwrap() < 600_000, "{numbers}");
    assert_ne!(numbers["helper_peak_watched_bytes"], json!(1));
    assert_eq!(numbers["kept"], json!(7), "its other numbers go on");
}
