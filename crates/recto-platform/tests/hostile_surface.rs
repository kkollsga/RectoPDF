//! What a compromised worker can do with a surface it was sent, across a real process boundary and the real link. The child here is this same test binary, started again.
#![cfg(target_os = "macos")]

use recto_platform::isolate::{Channel, ChildProcess};
use recto_platform::surface::{LinkInvite, PixelSurface, SurfaceReceiver};
use std::ffi::OsStr;
use std::time::{Duration, Instant};

const CHILD: &str = "RECTO_TEST_EMPTY_THE_SURFACE";

/// Failure: a worker calls `IOSurfaceSetPurgeable(Empty)` on a surface the host is about to read and write, as dark paper does to every tile. If that faulted, a compromised worker could crash the app from inside its sandbox, and turning tiles over would have to move into the worker. The host reads zeros or is told, and carries on.
#[test]
fn a_surface_a_worker_emptied_does_not_crash_the_host() {
    if std::env::var_os(CHILD).is_some() {
        let receiver = SurfaceReceiver::from_invite().expect("the link");
        let surface = receiver.next(Duration::from_secs(20)).expect("a surface");
        surface.empty_now();
        return;
    }
    let surface = PixelSurface::create(512, 512).unwrap();
    surface.with_pixels(|pixels| pixels.fill(0xAB)).unwrap();
    let (ours, theirs) = Channel::pair().unwrap();
    let invite = LinkInvite::new().unwrap();
    let exe = std::env::current_exe().unwrap();
    let args = [OsStr::new("--exact"), OsStr::new("a_surface_a_worker_emptied_does_not_crash_the_host")];
    let child = ChildProcess::spawn_linked(&exe, &args, &[(CHILD, "1")], theirs, None, Some(&invite)).unwrap();
    let link = invite.accept(Duration::from_secs(20)).expect("the child's link");
    link.send(&surface).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !surface.was_emptied() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(surface.was_emptied(), "the child emptied it");
    // What `engine.rs` does with a dark tile: reads and writes every pixel.
    let read = surface.with_pixels(|pixels| {
        let kept = pixels.iter().filter(|byte| **byte == 0xAB).count();
        let zero = pixels.iter().filter(|byte| **byte == 0).count();
        pixels.fill(0x11);
        (kept, zero, pixels.len())
    });
    println!("after the worker emptied it: {read:?}, emptied still: {}", surface.was_emptied());
    assert!(read.is_ok() || read.is_err(), "and it's still here to say so");
    drop(ours);
    drop(child);
}
