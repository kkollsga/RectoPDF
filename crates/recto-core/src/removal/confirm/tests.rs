use super::*;
use crate::removal::tests::{art, make, ok};
use crate::save::Target;
use recto_platform::FileStamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Person,
    Agent(u8),
}

const fn n(v: u8) -> NonZeroU8 {
    match NonZeroU8::new(v) {
        Some(n) => n,
        None => unreachable!(),
    }
}

const SMALL: ConfirmLimits = ConfirmLimits {
    max_pending: n(2),
    per_actor: n(1),
    ttl: Duration::from_secs(10),
};

fn stamp() -> FileStamp {
    FileStamp {
        size: 100,
        mtime_ns: 5,
        device: 1,
        inode: 9,
    }
}

fn version() -> Version {
    Version { epoch: 1, source: stamp() }
}

fn bind(actor: Who, target: TargetKey) -> Binding<Who> {
    Binding {
        actor,
        version: version(),
        target,
    }
}

fn src() -> &'static Path {
    Path::new("/doc.pdf")
}

fn same() -> TargetKey {
    TargetKey::new(src(), Target::Same)
}

fn new_file(name: &str) -> TargetKey {
    TargetKey::new(src(), Target::NewFile(Path::new(name)))
}

fn save_as_plan() -> Plan {
    make(TargetMode::SaveAs, false, &[], &crate::removal::DESIGN_LIMITS).unwrap()
}

fn store() -> Confirmations<Who> {
    Confirmations::new(SMALL)
}

/// Catches a cap that is off by one, or one that lets a third actor push out a pending plan.
#[test]
fn pending_cap_refuses_at_limit_plus_one() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let mut s = store();
    let p = s.offer(bind(Who::Person, same()), &plan, t0).unwrap();
    let a = s.offer(bind(Who::Agent(1), same()), &plan, t0).unwrap();
    assert_eq!(s.offer(bind(Who::Agent(2), same()), &plan, t0), Err(ConfirmRefusal::Full));
    assert_eq!(s.len(), 2);
    assert!(s.take(p, &bind(Who::Person, same()), &plan, t0).is_ok());
    assert!(s.take(a, &bind(Who::Agent(1), same()), &plan, t0).is_ok());
}

/// Catches an actor at the cap evicting another actor's plan (here the oldest of all), or keeping its own stale one instead.
#[test]
fn at_cap_an_actor_replaces_only_its_own_oldest() {
    let (plan, t0) = (save_as_plan(), Instant::now());
    let mut s = store();
    let others = s.offer(bind(Who::Agent(1), new_file("/a.pdf")), &plan, t0).unwrap();
    let first = s.offer(bind(Who::Person, new_file("/a.pdf")), &plan, t0).unwrap();
    let third = s.offer(bind(Who::Person, new_file("/b.pdf")), &plan, t0).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(
        s.take(first, &bind(Who::Person, new_file("/a.pdf")), &plan, t0).unwrap_err(),
        ConfirmRefusal::Unknown
    );
    assert!(s.take(others, &bind(Who::Agent(1), new_file("/a.pdf")), &plan, t0).is_ok());
    assert!(s.take(third, &bind(Who::Person, new_file("/b.pdf")), &plan, t0).is_ok());
}

/// Catches an actor holding more than its share: one actor at its share offering again must replace only its own oldest,
/// never another actor's plan, and never grow.
#[test]
fn an_actor_at_its_share_evicts_only_its_own_oldest() {
    let (plan, t0) = (save_as_plan(), Instant::now());
    let mut s = Confirmations::new(ConfirmLimits {
        max_pending: n(4),
        per_actor: n(2),
        ttl: SMALL.ttl,
    });
    let x = s.offer(bind(Who::Agent(1), new_file("/x.pdf")), &plan, t0).unwrap();
    let y = s.offer(bind(Who::Agent(1), new_file("/y.pdf")), &plan, t0).unwrap();
    let z = s.offer(bind(Who::Person, new_file("/z.pdf")), &plan, t0).unwrap();
    let w = s.offer(bind(Who::Agent(1), new_file("/w.pdf")), &plan, t0).unwrap();
    assert_eq!(s.len(), 3);
    assert_eq!(
        s.take(x, &bind(Who::Agent(1), new_file("/x.pdf")), &plan, t0).unwrap_err(),
        ConfirmRefusal::Unknown
    );
    assert!(s.take(y, &bind(Who::Agent(1), new_file("/y.pdf")), &plan, t0).is_ok());
    assert!(s.take(z, &bind(Who::Person, new_file("/z.pdf")), &plan, t0).is_ok());
    assert!(s.take(w, &bind(Who::Agent(1), new_file("/w.pdf")), &plan, t0).is_ok());
}

/// Catches one agent filling every slot and locking the person out of confirming anything: with the real limits an agent offering
/// 8 targets holds only its share, and the person's offer still succeeds.
#[test]
fn one_actor_cannot_fill_the_store_and_lock_out_the_person() {
    let (plan, t0) = (save_as_plan(), Instant::now());
    let mut s = Confirmations::new(CONFIRM_LIMITS);
    for i in 0..8 {
        s.offer(bind(Who::Agent(1), new_file(&format!("/t{i}.pdf"))), &plan, t0).unwrap();
    }
    assert_eq!(s.len(), usize::from(CONFIRM_LIMITS.per_actor.get()));
    assert!(s.offer(bind(Who::Person, new_file("/mine.pdf")), &plan, t0).is_ok());
}

/// Catches a share that is not enforced at the limit plus one: with a share of 1 and room for 2, a second target replaces the
/// actor's own plan (never `Full`, never a third entry), while an actor holding nothing is `Full` and pushes nobody out.
#[test]
fn the_share_holds_at_the_limit_plus_one() {
    let (plan, t0) = (save_as_plan(), Instant::now());
    let mut s = store();
    s.offer(bind(Who::Agent(1), new_file("/a.pdf")), &plan, t0).unwrap();
    s.offer(bind(Who::Agent(2), new_file("/a.pdf")), &plan, t0).unwrap();
    s.offer(bind(Who::Agent(1), new_file("/b.pdf")), &plan, t0).unwrap();
    assert_eq!(s.len(), 2);
    assert_eq!(s.offer(bind(Who::Person, new_file("/a.pdf")), &plan, t0), Err(ConfirmRefusal::Full));
    assert_eq!(s.len(), 2);
}

/// Catches two live confirmations for one actor and target: the person could confirm a plan older than the one on screen.
#[test]
fn a_new_plan_for_the_same_target_supersedes_the_old() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let mut s = store();
    let old = s.offer(bind(Who::Person, same()), &plan, t0).unwrap();
    let new = s.offer(bind(Who::Person, same()), &plan, t0).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s.take(old, &bind(Who::Person, same()), &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
    assert!(s.take(new, &bind(Who::Person, same()), &plan, t0).is_ok());
}

/// Catches a confirmation that outlives its ttl, one that expires early, a clock other than the one passed in,
/// and a full store that keeps refusing after its entries have expired.
#[test]
fn expiry_follows_the_injected_clock() {
    let (plan, t0, ttl) = (ok(&[]), Instant::now(), SMALL.ttl);
    let b = bind(Who::Person, same());
    let mut s = store();
    let id = s.offer(b, &plan, t0).unwrap();
    assert!(s.take(id, &b, &plan, t0 + ttl - Duration::from_nanos(1)).is_ok());
    let id = s.offer(b, &plan, t0).unwrap();
    assert_eq!(s.take(id, &b, &plan, t0 + ttl).unwrap_err(), ConfirmRefusal::Expired);
    assert_eq!(s.take(id, &b, &plan, t0 + ttl).unwrap_err(), ConfirmRefusal::Unknown);
    s.offer(b, &plan, t0).unwrap();
    s.offer(bind(Who::Agent(1), same()), &plan, t0).unwrap();
    assert_eq!(s.len(), 2);
    assert!(s.offer(bind(Who::Agent(2), same()), &plan, t0 + ttl).is_ok());
    assert_eq!(s.len(), 1);
}

/// Catches confirming a plan made for an older document: a changed epoch, or the same epoch over a changed file.
#[test]
fn stale_epoch_or_changed_file_refuses() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let b = bind(Who::Person, same());
    let other_epoch = Binding {
        version: Version { epoch: 2, ..version() },
        ..b
    };
    let other_file = Binding {
        version: Version {
            source: FileStamp { size: 101, ..stamp() },
            ..version()
        },
        ..b
    };
    for changed in [other_epoch, other_file] {
        let mut s = store();
        let id = s.offer(b, &plan, t0).unwrap();
        assert_eq!(s.take(id, &changed, &plan, t0).unwrap_err(), ConfirmRefusal::Stale);
        assert_eq!(s.take(id, &b, &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
    }
}

/// Catches one actor confirming, cancelling or learning about another's plan, and a wrong actor's
/// refusal removing the plan or revealing that it exists (an id held by another actor answers as an unknown id).
#[test]
fn other_actor_refuses_and_keeps_the_plan() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let person = bind(Who::Person, same());
    let mut s = store();
    let id = s.offer(person, &plan, t0).unwrap();
    for other in [Who::Agent(1), Who::Agent(2)] {
        assert_eq!(s.take(id, &bind(other, same()), &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
    }
    // Even stale, expired and for another target, the answer is the same as for an id that was never offered.
    let stale_late = Binding {
        version: Version { epoch: 9, ..version() },
        target: new_file("/x.pdf"),
        actor: Who::Agent(1),
    };
    let late = t0 + SMALL.ttl * 2;
    assert_eq!(s.take(id, &stale_late, &plan, late).unwrap_err(), ConfirmRefusal::Unknown);
    assert_eq!(s.len(), 1);
    assert!(s.take(id, &person, &plan, t0).is_ok());
    let agent = bind(Who::Agent(1), same());
    let id = s.offer(agent, &plan, t0).unwrap();
    assert_eq!(s.take(id, &bind(Who::Agent(2), same()), &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
    assert_eq!(s.len(), 1);
    assert!(s.take(id, &agent, &plan, t0).is_ok());
}

/// Catches a confirmation for one output being used for another: a different mode, output path, replace-versus-new, or source file.
#[test]
fn other_target_refuses() {
    let t0 = Instant::now();
    let save = ok(&[]);
    let save_as = save_as_plan();
    let cases: [(TargetKey, TargetKey, &Plan); 5] = [
        (same(), new_file("/a.pdf"), &save),
        (new_file("/a.pdf"), new_file("/b.pdf"), &save_as),
        (new_file("/a.pdf"), TargetKey::new(src(), Target::Replace(Path::new("/a.pdf"))), &save_as),
        (same(), TargetKey::new(Path::new("/other.pdf"), Target::Same), &save),
        (
            new_file("/a.pdf"),
            TargetKey::new(Path::new("/other.pdf"), Target::NewFile(Path::new("/a.pdf"))),
            &save_as,
        ),
    ];
    for (offered, taken, plan) in cases {
        let mut s = store();
        let id = s.offer(bind(Who::Person, offered), plan, t0).unwrap();
        assert_eq!(s.take(id, &bind(Who::Person, taken), plan, t0).unwrap_err(), ConfirmRefusal::OtherTarget);
        assert!(s.is_empty());
    }
}

/// Catches a plan kept for an output of another mode, which no take could match as the plan's own.
#[test]
fn mode_mismatch_refuses_offer() {
    let t0 = Instant::now();
    let mut s = store();
    assert_eq!(s.offer(bind(Who::Person, same()), &save_as_plan(), t0), Err(ConfirmRefusal::Mismatched));
    assert_eq!(s.offer(bind(Who::Person, new_file("/a.pdf")), &ok(&[]), t0), Err(ConfirmRefusal::Mismatched));
    assert!(s.is_empty());
}

/// Catches a confirmation carrying over to a plan that was rebuilt with more to remove after it was shown.
#[test]
fn changed_plan_refuses() {
    let t0 = Instant::now();
    let b = bind(Who::Person, same());
    let mut s = store();
    let id = s.offer(b, &ok(&[]), t0).unwrap();
    let more = ok(&[art(1, Store::Journal, crate::removal::Owner::Target)]);
    assert_eq!(s.take(id, &b, &more, t0).unwrap_err(), ConfirmRefusal::PlanChanged);
    assert!(s.is_empty());
}

/// Catches one confirmation starting two removals.
#[test]
fn a_confirmation_is_used_once() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let b = bind(Who::Person, same());
    let mut s = store();
    let id = s.offer(b, &plan, t0).unwrap();
    let done = s.take(id, &b, &plan, t0).unwrap();
    assert_eq!(done.digest(), plan.digest());
    assert_eq!(s.take(id, &b, &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
}

/// Catches an id coming back after its plan went (taken, replaced or evicted), so a late answer to
/// an old question could confirm a new plan; and a counter that runs out panicking.
#[test]
fn ids_are_never_reused() {
    let (plan, t0) = (ok(&[]), Instant::now());
    let b = bind(Who::Person, same());
    let mut s = store();
    let first = s.offer(b, &plan, t0).unwrap();
    assert_eq!(first.get(), 1);
    s.take(first, &b, &plan, t0).unwrap();
    let second = s.offer(b, &plan, t0).unwrap();
    let third = s.offer(b, &plan, t0).unwrap();
    assert_eq!((second.get(), third.get()), (2, 3));
    assert_eq!(s.take(first, &b, &plan, t0).unwrap_err(), ConfirmRefusal::Unknown);
    s.next = u64::MAX;
    assert_eq!(s.offer(b, &plan, t0), Err(ConfirmRefusal::Full));
}

/// Catches a store that holds memory for a feature nobody has used (rule 1: a cold tab costs nothing).
#[test]
fn an_empty_store_holds_no_buffer() {
    let s = store();
    assert_eq!(s.pending.capacity(), 0);
}
