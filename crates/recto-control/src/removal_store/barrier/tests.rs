//! The record store on this Mac's disk, in process: each test names the failure it catches. A kill is a hook's `Stop`: nothing after it runs, nothing is cleaned.

use super::*;
use crate::removal_store::testing::{Restore, limits, mkfifo, mode, version};
use recto_core::removal::{
    Actor, ArtifactKey, Binding, CONFIRM_LIMITS, Class, Confirmations, Format, Found, Kinds, LEDGER_LIMITS, Owner, Pin, PinKind, PlanId, Request, Store,
    Survey, TargetKey, TargetMode, Version, encode_header, lineage, plan, read_ledger,
};
use recto_core::save::Target;
use recto_testpdf::{TempDir, returns_in_time};
use std::os::unix::fs::PermissionsExt as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn temp() -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-barrier-")
}

fn found(store: Store, name: &str, owner: Owner) -> Found {
    Found {
        store,
        key: ArtifactKey::of(store, name.as_bytes()),
        owner,
        format: Format::Current,
    }
}

struct Rig {
    surveyed: Surveyed,
    limits: Limits,
    plan: Plan,
    facts: BarrierFacts,
}

/// A removal of a journal, a writer's temporary and a session record, with another document's journal beside them.
fn rig() -> Rig {
    rig_named("")
}

/// The same shape of removal over other files: another plan.
fn rig_named(suffix: &str) -> Rig {
    let limits = limits(8);
    let mut survey = Survey::new(&limits);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
    let ledger = read_ledger(&encode_header(1_000), &LEDGER_LIMITS);
    survey.history(&lineage(&ledger, version(5), Path::new("/d/a.pdf"), &LEDGER_LIMITS).unwrap());
    for f in [
        found(Store::Journal, &format!("j1{suffix}"), Owner::Target),
        found(Store::WriterTemp, &format!("t1{suffix}"), Owner::Target),
        found(Store::SessionRecord, &format!("s1{suffix}"), Owner::Target),
        found(Store::Journal, "other", Owner::Unrelated),
    ] {
        survey.add(f).unwrap();
    }
    let surveyed = survey.finish().unwrap();
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false);
    let plan = plan(&request, &surveyed.inventory(), &limits).unwrap();
    let pins = surveyed
        .inventory()
        .artifacts()
        .iter()
        .map(|a| {
            (a.owner == Owner::Target && matches!(a.store.class(), Class::Purge | Class::Rewrite | Class::Trash)).then_some(Pin {
                root: 0,
                inode: 100 + u64::from(a.id.0),
                kind: PinKind::File,
            })
        })
        .collect();
    let facts = BarrierFacts {
        target: "/d/a.pdf".into(),
        source: version(5),
        candidate_name: ".a.pdf.new-1".into(),
        candidate: version(6),
        candidate_sha: [9; 32],
        roots: vec![[1; 16]],
        actor: Actor::Person,
        pins,
    };
    Rig { surveyed, limits, plan, facts }
}

impl Rig {
    /// A new walk of the plan, confirmed and acknowledged up to the barrier.
    fn progress(&self) -> Progress {
        let mut confirmations = Confirmations::new(CONFIRM_LIMITS);
        let binding = Binding {
            actor: 0u8,
            version: Version {
                epoch: 1,
                source: recto_platform::FileStamp {
                    size: 1,
                    mtime_ns: 2,
                    device: 3,
                    inode: 4,
                },
            },
            target: TargetKey::new(Path::new("/doc.pdf"), Target::Same),
        };
        let now = Instant::now();
        let id = confirmations.offer(binding, &self.plan, now, PlanId::from_raw(9).unwrap()).unwrap();
        let confirmed = confirmations.take(id, &binding, &self.plan, now).unwrap();
        let mut progress = Progress::start(&self.plan, confirmed).unwrap();
        for i in 0..self.base() {
            progress.acknowledge(&self.plan, i, Ack::Done).unwrap();
        }
        progress
    }

    fn base(&self) -> u32 {
        self.plan.steps().iter().position(|s| s.effect() == Effect::PersistBarrier).unwrap() as u32
    }

    fn last(&self) -> u32 {
        self.plan.steps().len() as u32 - 1
    }

    fn begin(&self, root: &Path, progress: &mut Progress, opts: &Opts<'_>) -> Result<Held, BeginError> {
        begin_with(root, &self.plan, &self.surveyed, &self.limits, progress, &self.facts, opts)
    }

    fn begin_ok(&self, root: &Path) -> (Held, Progress) {
        let mut progress = self.progress();
        let held = self.begin(root, &mut progress, &Opts::standard()).unwrap();
        (held, progress)
    }

    fn len(&self) -> usize {
        let progress = self.progress();
        encode_barrier_within(&self.plan, &self.surveyed, &self.limits, &progress, &self.facts, MAX_BARRIER_BYTES)
            .unwrap()
            .bytes
            .len()
    }
}

fn path(root: &Path) -> std::path::PathBuf {
    root.join(DIR).join(NAME)
}

fn stop_at(point: Point) -> impl Fn(Point) -> Flow {
    move |p| if p == point { Flow::Stop } else { Flow::Go }
}

fn pending(root: &Path) -> Box<Recorded> {
    match look(root) {
        Looked::Pending(r) => r,
        other => panic!("{other:?}"),
    }
}

fn names(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root.join(DIR))
        .map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Catches a kill at any point of `begin` leaving a state that is neither safe to go on from nor to start again: before the name is made nothing is pending and
/// the next begin sweeps the leftover and succeeds; from the rename to slot 0 the record is pending with nothing done and may be abandoned; once slot 0 is
/// there it resumes and may not be abandoned.
#[test]
fn a_kill_at_each_point_of_begin_leaves_a_state_that_can_be_gone_on_from() {
    let r = rig();
    for point in [
        Point::Swept,
        Point::TempMade,
        Point::TempWritten,
        Point::TempSynced,
        Point::Renamed,
        Point::FolderSynced,
        Point::ReadBack,
        Point::Slot0Written,
        Point::Slot0Synced,
    ] {
        let dir = temp();
        let mut progress = r.progress();
        let stopper = stop_at(point);
        let got = r.begin(
            dir.path(),
            &mut progress,
            &Opts {
                cap: MAX_BARRIER_BYTES,
                at: &stopper,
            },
        );
        assert_eq!(got.unwrap_err(), BeginError::Stopped, "{point:?}");
        assert_eq!(progress.acked(), r.base(), "{point:?}: the barrier was acknowledged by a begin that was killed");
        match point {
            Point::Swept | Point::TempMade | Point::TempWritten | Point::TempSynced => {
                assert!(matches!(look(dir.path()), Looked::Nothing), "{point:?}");
                assert_eq!(
                    names(dir.path()).len(),
                    usize::from(point != Point::Swept),
                    "{point:?}: the temporary is left, as a kill leaves it"
                );
                let (_held, _) = r.begin_ok(dir.path());
                assert_eq!(names(dir.path()), [NAME], "{point:?}: the next begin sweeps the temporary");
            }
            Point::Renamed | Point::FolderSynced | Point::ReadBack => {
                let rec = pending(dir.path());
                assert_eq!(rec.outcome(), Outcome::Unchanged, "{point:?}");
                assert_eq!(rec.resume().unwrap_err(), OrderError::NotResumable);
                abandon(dir.path(), &rec).unwrap();
                assert!(matches!(look(dir.path()), Looked::Nothing));
                r.begin_ok(dir.path());
            }
            Point::Slot0Written | Point::Slot0Synced => {
                let rec = pending(dir.path());
                assert_eq!(rec.acked(), r.base() + 1, "{point:?}");
                let (plan, walk) = rec.resume().unwrap();
                assert_eq!(walk.next(&plan), Some(plan.steps()[r.base() as usize + 1]));
                assert_eq!(abandon(dir.path(), &rec).unwrap_err(), AbandonError::RecoveryRequired);
            }
            _ => unreachable!(),
        }
    }
}

/// Catches a restart repeating a step that was acknowledged, or skipping one that was not: the walk is killed after doing step `k` and before its slot, for each
/// `k`, and then resumed from `look`. Every step is done at least once, and none before the last slot is done again.
#[test]
fn a_walk_killed_at_every_step_resumes_at_exactly_the_next_one() {
    let r = rig();
    for k in r.base() + 1..=r.last() {
        let dir = temp();
        let mut done: Vec<u32> = (0..=r.base()).collect();
        let (held, mut progress) = r.begin_ok(dir.path());
        for i in r.base() + 1..k {
            done.push(i);
            held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap();
        }
        done.push(k); // the step is done; the kill comes before it is acknowledged
        drop((held, progress));

        let rec = pending(dir.path());
        assert_eq!(rec.acked(), k, "the highest slot is the last one written");
        let (plan, mut progress) = rec.resume().unwrap();
        let held = reopen(dir.path(), &rec).unwrap();
        while let Some(step) = progress.next(&plan) {
            let i = progress.acked();
            done.push(i);
            if step.effect() == Effect::RetireBarrier {
                break;
            }
            held.acknowledge(&plan, &mut progress, i, Ack::Done).unwrap();
        }
        // Everything up to the barrier, then each step once up to `k` (which was done and not acknowledged), then `k` again and on to the last: no step
        // before the last slot runs twice.
        let expected: Vec<u32> = (0..=r.base()).chain(r.base() + 1..=k).chain(k..=r.last()).collect();
        assert_eq!(done, expected, "k {k}");
        let mut walk = progress;
        held.retire(&plan, &mut walk).unwrap();
        assert!(walk.succeeded(&plan));
        assert!(matches!(look(dir.path()), Looked::Nothing), "the record is gone with the last step");
    }
}

/// Catches a step that fails writing a slot, and a step acknowledged as failed writing one: neither moves the highest slot.
#[test]
fn a_failed_step_writes_no_slot_and_stops_the_walk() {
    let r = rig();
    let dir = temp();
    let (held, mut progress) = r.begin_ok(dir.path());
    let i = r.base() + 1;
    held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap();
    assert_eq!(
        held.acknowledge(&r.plan, &mut progress, i + 1, Ack::Failed),
        Ok(progress.outcome(&r.plan).unwrap())
    );
    assert_eq!(pending(dir.path()).acked(), i + 1, "a failure wrote a slot");
    assert!(progress.next(&r.plan).is_none());
    // Out of order, the last step and a step the walk is not at.
    let (held, mut progress) = r.begin_ok(temp().path());
    assert_eq!(
        held.acknowledge(&r.plan, &mut progress, r.base() + 3, Ack::Done).unwrap_err(),
        AckError::Order(OrderError::OutOfOrder)
    );
    assert_eq!(held.layout().slots(), r.last() - r.base());
}

/// Catches the record's file being replaced between `begin` and a step's slot: a copy under the name (another inode) is `Moved`, neither file is written, and
/// the step is acknowledged as failed.
#[test]
fn a_file_moved_and_replaced_by_a_copy_is_moved_and_neither_is_written() {
    let r = rig();
    let dir = temp();
    let (held, mut progress) = r.begin_ok(dir.path());
    let original = path(dir.path());
    let aside = dir.join(DIR).join("aside");
    std::fs::rename(&original, &aside).unwrap();
    std::fs::copy(&aside, &original).unwrap();
    let (before_a, before_b) = (std::fs::read(&aside).unwrap(), std::fs::read(&original).unwrap());
    let i = r.base() + 1;
    assert_eq!(held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap_err(), AckError::Moved);
    assert_eq!((std::fs::read(&aside).unwrap(), std::fs::read(&original).unwrap()), (before_a, before_b));
    assert!(progress.next(&r.plan).is_none(), "the step was not acknowledged as failed");
}

/// Catches a link at `removal/` being read through to a record somewhere else, and a file there being called anything but not ours.
#[test]
fn removal_as_a_link_or_a_file_is_not_ours() {
    let r = rig();
    let real = temp();
    r.begin_ok(real.path());
    let root = temp();
    std::os::unix::fs::symlink(real.join(DIR), root.join(DIR)).unwrap();
    assert!(matches!(look(root.path()), Looked::NotOurs));
    let mut progress = r.progress();
    assert_eq!(r.begin(root.path(), &mut progress, &Opts::standard()).unwrap_err(), BeginError::NotOurs);
    let root = temp();
    std::fs::write(root.join(DIR), b"x").unwrap();
    assert!(matches!(look(root.path()), Looked::NotOurs));
    assert!(matches!(look(temp().path()), Looked::Nothing));
}

/// Catches anything but a private plain file with one name at `pending.v1` being read or called nothing: a link, a pipe (which must not be waited on), a
/// folder, a second hard link, a file others can read, one over the cap. Each is `Damaged`, and is left where it is.
#[test]
fn what_is_at_the_name_but_a_private_plain_file_is_damaged() {
    let r = rig();
    let make = |f: &dyn Fn(&Path)| {
        let root = temp();
        r.begin_ok(root.path());
        f(&path(root.path()));
        root
    };
    let cases: Vec<(&str, TempDir)> = vec![
        (
            "a link",
            make(&|p| {
                let aside = p.with_file_name("aside");
                std::fs::rename(p, &aside).unwrap();
                std::os::unix::fs::symlink(&aside, p).unwrap();
            }),
        ),
        (
            "a pipe",
            make(&|p| {
                std::fs::remove_file(p).unwrap();
                mkfifo(p);
            }),
        ),
        (
            "a folder",
            make(&|p| {
                std::fs::remove_file(p).unwrap();
                std::fs::create_dir(p).unwrap();
            }),
        ),
        ("a second hard link", make(&|p| std::fs::hard_link(p, p.with_file_name("other")).unwrap())),
        ("mode 0644", make(&|p| mode(p, 0o644))),
        ("mode 0666", make(&|p| mode(p, 0o666))),
    ];
    for (what, root) in cases {
        let looked = returns_in_time(what, {
            let root = root.path().to_owned();
            move || look(&root)
        });
        assert!(matches!(looked, Looked::Damaged), "{what}: {looked:?}");
        assert!(std::fs::symlink_metadata(path(root.path())).is_ok(), "{what}: taken away");
    }
    // A file over the cap: only the cap's bytes and one are read, so a file of 64 GiB (sparse) answers at once.
    let root = temp();
    r.begin_ok(root.path());
    std::fs::OpenOptions::new()
        .write(true)
        .open(path(root.path()))
        .unwrap()
        .set_len(1 << 36)
        .unwrap();
    let looked = returns_in_time("a huge file", {
        let root = root.path().to_owned();
        move || look(&root)
    });
    assert!(matches!(looked, Looked::Damaged));
    // And a record that doesn't decode.
    let root = temp();
    r.begin_ok(root.path());
    let at = std::fs::metadata(path(root.path())).unwrap().len() as usize - 1;
    let mut bytes = std::fs::read(path(root.path())).unwrap();
    bytes[40] ^= 1;
    std::fs::write(path(root.path()), &bytes[..at]).unwrap();
    assert!(matches!(look(root.path()), Looked::Damaged));
}

/// Catches a later version's record being cleared or called damaged, which a newer app would then lose.
#[test]
fn a_later_version_is_too_new() {
    let r = rig();
    let root = temp();
    r.begin_ok(root.path());
    let mut bytes = std::fs::read(path(root.path())).unwrap();
    bytes[16] = 2;
    std::fs::write(path(root.path()), &bytes).unwrap();
    assert!(matches!(look(root.path()), Looked::TooNew));
}

/// Catches a leftover temporary that is not a plain file of ours (a link, a pipe, a folder, shaped like ours) being taken away or walked into: it is `NotOurs`,
/// left alone, and nothing is begun.
#[test]
fn a_temporary_shaped_thing_that_is_not_a_file_is_not_ours_and_is_left() {
    let r = rig();
    let shaped = format!("{TEMP_PREFIX}0123456789abcdef");
    for make in [
        (|p: &Path| std::os::unix::fs::symlink("/nowhere", p).unwrap()) as fn(&Path),
        |p| mkfifo(p),
        |p| std::fs::create_dir(p).unwrap(),
    ] {
        let root = temp();
        local::ensure_private_dir(&root.join(DIR)).unwrap();
        make(&root.join(DIR).join(&shaped));
        let mut progress = r.progress();
        assert_eq!(r.begin(root.path(), &mut progress, &Opts::standard()).unwrap_err(), BeginError::NotOurs);
        assert!(std::fs::symlink_metadata(root.join(DIR).join(&shaped)).is_ok(), "left alone");
        assert_eq!(names(root.path()), std::slice::from_ref(&shaped), "nothing was begun");
        assert!(matches!(look(root.path()), Looked::Nothing), "a temporary is never pending");
    }
    // One that only starts like ours is no temporary of ours, and is not touched.
    let root = temp();
    local::ensure_private_dir(&root.join(DIR)).unwrap();
    std::fs::write(root.join(DIR).join(format!("{TEMP_PREFIX}notours")), b"x").unwrap();
    r.begin_ok(root.path());
    assert_eq!(names(root.path()).len(), 2);
}

/// Catches two begins at once both making a record, or one leaving a half-written file: with both past their sweeps before either makes a temporary, exactly one wins,
/// the other is told `Pending` and takes only its own temporary away, and the file decodes whole.
#[test]
fn two_begins_at_once_make_one_record() {
    let r = rig();
    let root = temp();
    let both = std::sync::Barrier::new(2);
    let results: Vec<Result<Held, BeginError>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    let mut progress = r.progress();
                    let at = |p: Point| {
                        if p == Point::Swept {
                            both.wait();
                        }
                        Flow::Go
                    };
                    r.begin(
                        root.path(),
                        &mut progress,
                        &Opts {
                            cap: MAX_BARRIER_BYTES,
                            at: &at,
                        },
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().any(|r| matches!(r, Err(BeginError::Pending))));
    assert_eq!(pending(root.path()).acked(), r.base() + 1);
    assert_eq!(names(root.path()), [NAME], "the loser's temporary is gone");
}

/// Catches a sweep (the next begin's) taking a running begin's temporary and leaving two records or a torn one: the running begin fails, the new one is whole,
/// and nothing else is left.
#[test]
fn a_sweep_racing_a_begin_never_makes_two_records_or_a_torn_one() {
    let r = rig();
    let root = temp();
    let (ready_tx, ready) = mpsc::channel();
    let (go_tx, go) = mpsc::channel::<()>();
    let go = std::sync::Mutex::new(go);
    std::thread::scope(|s| {
        let slow = s.spawn(|| {
            let mut progress = r.progress();
            let at = |p: Point| {
                if p == Point::TempWritten {
                    ready_tx.send(()).unwrap();
                    go.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Flow::Go
            };
            r.begin(
                root.path(),
                &mut progress,
                &Opts {
                    cap: MAX_BARRIER_BYTES,
                    at: &at,
                },
            )
            .map(|_| ())
        });
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let (_held, _) = r.begin_ok(root.path());
        go_tx.send(()).unwrap();
        let slow = slow.join().unwrap();
        assert!(slow.is_err(), "the swept begin made a record: {slow:?}");
    });
    assert_eq!(names(root.path()), [NAME]);
    assert_eq!(pending(root.path()).acked(), r.base() + 1);
}

/// Catches a folder that can't be written being begun in anyway, or leaving a record or temporary behind.
#[test]
fn a_folder_that_cant_be_written_gives_an_error_and_nothing() {
    let r = rig();
    let root = temp();
    local::ensure_private_dir(&root.join(DIR)).unwrap();
    mode(&root.join(DIR), 0o500);
    let _restore = Restore(root.join(DIR));
    let mut progress = r.progress();
    assert_eq!(
        r.begin(root.path(), &mut progress, &Opts::standard()).unwrap_err(),
        BeginError::Io(io::ErrorKind::PermissionDenied)
    );
    assert!(names(root.path()).is_empty());
    assert_eq!(progress.acked(), r.base());
}

/// Catches a record that was damaged between its write and the read-back being trusted: the barrier is not acknowledged and the file is taken away (no slot
/// exists, so nothing was purged).
#[test]
fn a_byte_changed_before_the_read_back_fails_the_begin_and_takes_the_file_away() {
    let r = rig();
    let root = temp();
    let file = path(root.path());
    let at = |p: Point| {
        if p == Point::FolderSynced {
            let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
            f.write_all_at(&[0x55], 100).unwrap();
        }
        Flow::Go
    };
    let mut progress = r.progress();
    assert_eq!(
        r.begin(
            root.path(),
            &mut progress,
            &Opts {
                cap: MAX_BARRIER_BYTES,
                at: &at
            }
        )
        .unwrap_err(),
        BeginError::ReadBack
    );
    assert!(names(root.path()).is_empty());
    assert_eq!(progress.acked(), r.base());
}

/// Catches the cap being checked after the file is made, or off by one: a record exactly at a small cap is made and one byte over is refused with the folder
/// still without a file.
#[test]
fn a_record_at_the_cap_is_made_and_one_over_makes_nothing() {
    let r = rig();
    let len = r.len();
    let (root, over) = (temp(), temp());
    let mut progress = r.progress();
    r.begin(root.path(), &mut progress, &Opts { cap: len, at: &|_| Flow::Go }).unwrap();
    assert!(matches!(look_with(root.path(), len), Looked::Pending(_)));
    assert!(matches!(look_with(root.path(), len - 1), Looked::Damaged), "a record over the reader's cap");
    let mut progress = r.progress();
    assert_eq!(
        r.begin(
            over.path(),
            &mut progress,
            &Opts {
                cap: len - 1,
                at: &|_| Flow::Go
            }
        )
        .unwrap_err(),
        BeginError::Refused(BarrierRefusal::TooLarge)
    );
    assert!(names(over.path()).is_empty());
}

/// Catches a state folder others can enter, or one that Time Machine copies, or a record others can read.
#[test]
fn the_folder_is_private_and_out_of_backups_and_the_file_is_0600() {
    let r = rig();
    let root = temp();
    r.begin_ok(root.path());
    let dir = std::fs::metadata(root.join(DIR)).unwrap().permissions().mode();
    assert_eq!(dir & 0o777, 0o700);
    assert!(local::is_excluded_from_backups(&root.join(DIR)));
    assert_eq!(std::fs::metadata(path(root.path())).unwrap().permissions().mode() & 0o777, 0o600);
}

/// Catches a second record being begun over a pending one, and a record being abandoned that is not the one on disk.
#[test]
fn one_transaction_at_a_time_and_abandon_is_for_the_record_given() {
    let r = rig();
    let root = temp();
    let (_held, _) = r.begin_ok(root.path());
    let mut progress = r.progress();
    assert_eq!(r.begin(root.path(), &mut progress, &Opts::standard()).unwrap_err(), BeginError::Pending);
    let rec = pending(root.path());
    assert_eq!(abandon(root.path(), &rec).unwrap_err(), AbandonError::RecoveryRequired);
    assert!(path(root.path()).exists(), "an abandon that was refused took the file");
    // A real record that is another one than the one given: both before slot 0 (so only the mismatch can stop the abandon), the other of another plan.
    let (mine, other) = (temp(), temp());
    let stopper = stop_at(Point::ReadBack);
    let mut p = r.progress();
    r.begin(
        mine.path(),
        &mut p,
        &Opts {
            cap: MAX_BARRIER_BYTES,
            at: &stopper,
        },
    )
    .unwrap_err();
    let given = pending(mine.path());
    let r2 = rig_named("-other");
    let mut p = r2.progress();
    r2.begin(
        other.path(),
        &mut p,
        &Opts {
            cap: MAX_BARRIER_BYTES,
            at: &stopper,
        },
    )
    .unwrap_err();
    let before = std::fs::read(path(other.path())).unwrap();
    assert_eq!(abandon(other.path(), &given).unwrap_err(), AbandonError::Gone);
    assert!(std::fs::read(path(other.path())).unwrap() == before, "another record was taken away");
    abandon(mine.path(), &given).unwrap();
}

/// Catches a slot being made durable for a step the walk may not take: a walk stopped by a failure, a walk of another plan, and a retire handed another plan's
/// walk. Each is refused before anything is written, so a later look never finds an acknowledgment nobody gave.
#[test]
fn nothing_is_written_for_a_stopped_walk_or_another_plan() {
    let r = rig();
    let other = rig_named("-other");
    assert_ne!(r.plan.digest(), other.plan.digest());
    let dir = temp();
    let (held, mut progress) = r.begin_ok(dir.path());
    let before = std::fs::read(path(dir.path())).unwrap();
    let i = r.base() + 1;
    assert!(held.acknowledge(&r.plan, &mut progress, i, Ack::Failed).is_ok());
    assert_eq!(
        held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap_err(),
        AckError::Order(OrderError::Stopped)
    );
    assert!(std::fs::read(path(dir.path())).unwrap() == before, "a stopped walk got a slot");

    let (held, _) = r.begin_ok(temp().path());
    let mut foreign = other.progress();
    let dir = temp();
    let (held2, _) = r.begin_ok(dir.path());
    let before = std::fs::read(path(dir.path())).unwrap();
    drop(held);
    assert_eq!(
        held2.acknowledge(&other.plan, &mut foreign, other.base(), Ack::Done).unwrap_err(),
        AckError::Order(OrderError::WrongPlan)
    );
    assert!(std::fs::read(path(dir.path())).unwrap() == before, "another plan's walk got a slot");

    // Another plan's walk at its own last step.
    let mut at_retire = other.progress();
    for i in other.base()..other.last() {
        at_retire.acknowledge(&other.plan, i, Ack::Done).unwrap();
    }
    assert_eq!(held2.retire(&other.plan, &mut at_retire).unwrap_err(), AckError::Order(OrderError::WrongPlan));
    assert!(path(dir.path()).exists(), "another plan's walk retired this record");
}

/// Catches a failed begin taking away whatever is at `pending.v1` by name: after the rename a thing that isn't the file made (another file put there,
/// not private) is not ours to delete, so the error is returned and it is still there.
#[test]
fn a_failed_begin_leaves_what_is_not_the_file_it_made() {
    let r = rig();
    let root = temp();
    let file = path(root.path());
    let at = |p: Point| {
        if p == Point::FolderSynced {
            std::fs::rename(&file, file.with_file_name("aside")).unwrap();
            std::fs::write(&file, b"someone else's").unwrap();
            mode(&file, 0o644);
        }
        Flow::Go
    };
    let mut progress = r.progress();
    assert!(
        r.begin(
            root.path(),
            &mut progress,
            &Opts {
                cap: MAX_BARRIER_BYTES,
                at: &at
            }
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"someone else's", "a file that was not ours was taken away");
    assert_eq!(progress.acked(), r.base());
}

/// Catches slot 0 being written to, or the barrier acknowledged for, a file the name no longer leads to: swapped before the slot is written, or after it is
/// flushed, `begin` is `NotDurable` and the barrier is not acknowledged.
#[test]
fn slot_0_needs_the_name_to_still_lead_to_the_file() {
    let r = rig();
    for point in [Point::ReadBack, Point::Slot0Synced] {
        let root = temp();
        let file = path(root.path());
        let aside = file.with_file_name("aside");
        let at = |p: Point| {
            if p == point {
                std::fs::rename(&file, &aside).unwrap();
                std::fs::copy(&aside, &file).unwrap();
            }
            Flow::Go
        };
        let mut progress = r.progress();
        assert_eq!(
            r.begin(
                root.path(),
                &mut progress,
                &Opts {
                    cap: MAX_BARRIER_BYTES,
                    at: &at
                }
            )
            .unwrap_err(),
            BeginError::NotDurable,
            "{point:?}"
        );
        assert_eq!(progress.acked(), r.base(), "{point:?}: the barrier was acknowledged");
        if point == Point::ReadBack {
            for f in [&file, &aside] {
                let rec = match decode_barrier_within(&std::fs::read(f).unwrap(), MAX_BARRIER_BYTES) {
                    Decoded::Pending(rec) => rec,
                    other => panic!("{other:?}"),
                };
                assert_eq!(rec.acked(), r.base(), "slot 0 was written to a file the name did not lead to");
            }
        }
    }
}

/// Catches a kill while a later slot is written, or while the record is retired, leaving a state a restart can't go on from: at each step's slot (written, and
/// flushed) the record shows that step acknowledged and resumes at the one after it, though the walk that was killed had not acknowledged it; a kill during the
/// retire leaves the record gone.
#[test]
fn a_kill_at_each_later_slot_and_at_the_retire_is_gone_on_from() {
    let r = rig();
    for k in r.base() + 1..r.last() {
        for synced in [false, true] {
            let dir = temp();
            let (held, mut progress) = r.begin_ok(dir.path());
            for i in r.base() + 1..k {
                held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap();
            }
            let stopper = stop_at(if synced { Point::SlotSynced(k + 1) } else { Point::SlotWritten(k + 1) });
            let got = held.acknowledge_with(
                &r.plan,
                &mut progress,
                k,
                Ack::Done,
                &Opts {
                    cap: MAX_BARRIER_BYTES,
                    at: &stopper,
                },
            );
            assert_eq!(got.unwrap_err(), AckError::Stopped, "k {k}");
            assert_eq!(progress.acked(), k, "k {k}: the walk was acknowledged by a step that was killed");
            let rec = pending(dir.path());
            assert_eq!(rec.acked(), k + 1, "k {k} synced {synced}");
            let (plan, walk) = rec.resume().unwrap();
            assert_eq!(walk.next(&plan), Some(plan.steps()[k as usize + 1]));
        }
    }
    for point in [Point::Unlinked, Point::UnlinkSynced] {
        let dir = temp();
        let (held, mut progress) = r.begin_ok(dir.path());
        for i in r.base() + 1..r.last() {
            held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap();
        }
        let stopper = stop_at(point);
        let got = held.retire_with(
            &r.plan,
            &mut progress,
            &Opts {
                cap: MAX_BARRIER_BYTES,
                at: &stopper,
            },
        );
        assert_eq!(got.unwrap_err(), AckError::Stopped, "{point:?}");
        assert_eq!(progress.acked(), r.last(), "{point:?}: retired by a kill");
        assert!(matches!(look(dir.path()), Looked::Nothing), "{point:?}");
    }
}
