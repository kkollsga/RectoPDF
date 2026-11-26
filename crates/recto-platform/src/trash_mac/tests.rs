//! The unit tests touch no Trash. The probe at the end attaches a disk image of its own and moves a file of its own into that volume's Trash.

use super::*;
use recto_testpdf::TempDir;
use std::os::unix::fs::MetadataExt;

/// Failure: a path that isn't there is "has a Trash" or is moved (a made-up name given to the system), or the call panics on it.
#[test]
fn a_missing_path_has_no_trash_and_moves_nothing() {
    let dir = TempDir::new("recto-trash-");
    let missing = dir.join("nothing");
    assert!(!has_trash_for(&missing));
    assert_eq!(move_to_trash(&missing), Err(Failure::BadPath));
    assert!(!has_trash_for(Path::new("/with\0nul")));
}

/// Every image this probe attached, detached on drop (a failed assertion included), by the mount folder it made, and only those.
struct Attached(Vec<PathBuf>);

impl Drop for Attached {
    fn drop(&mut self) {
        for mount in self.0.drain(..) {
            let plain = std::process::Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
            if !plain.is_ok_and(|s| s.success()) {
                let _ = std::process::Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mount).status();
            }
        }
    }
}

fn run(program: &str, args: &[&std::ffi::OsStr]) {
    let out = std::process::Command::new(program).args(args).output().expect("run");
    assert!(
        out.status.success(),
        "{program} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The measurement behind "the volume's own Trash, and no Trash means nothing is deleted" (design §10, D5). `--ignored`; makes two 20 MiB HFS+ images in
/// `$RECTO_PROBE_DIR` (or the temp folder), attaches each at a mount folder of its own (the second one read-only, after a file was put on it), and detaches
/// both in a drop guard. It moves only files it made on those images, and prints what it found.
///
/// **Result, 2026-11-26, macOS 27.0.1, HFS+ images, under load:** on a writable image with no Trash yet, `URLForDirectory:NSTrashDirectory` answers no with `create:NO`
/// and with `create:YES`, yet the move succeeded, made `<mount>/.Trashes/501/`, and the item kept its device and inode (renamed, not copied); after that the
/// question answers yes. So `has_trash_for` also accepts a local volume mounted writable. On the read-only image the question is no, the move is refused
/// (`NSError` code 3328, `NSFileWriteVolumeReadOnlyError`) and the file is still there. Not measured: a network share, an external physical disk, APFS (its image
/// attached the first time and then wouldn't attach a second time here, `EAGAIN`, so HFS+ was used).
#[test]
#[ignore = "attaches a disk image"]
fn trash_on_a_volume_of_its_own() {
    let base = std::env::var_os("RECTO_PROBE_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let dir = TempDir::new_in(&base, "recto-trash-probe-");
    let os = |s: &str| std::ffi::OsString::from(s);
    let mut attached = Attached(Vec::new());
    let ident = |p: &Path| std::fs::symlink_metadata(p).map(|m| (m.dev(), m.ino())).ok();
    let attach = |attached: &mut Attached, image: &Path, mount: &Path, readonly: bool| {
        run("mkdir", &[mount.as_os_str()]);
        let mut args = vec![
            os("attach"),
            os("-nobrowse"),
            os("-noautoopen"),
            os("-mountpoint"),
            mount.as_os_str().to_owned(),
        ];
        if readonly {
            args.push(os("-readonly"));
        }
        args.push(image.as_os_str().to_owned());
        let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
        run("hdiutil", &refs);
        attached.0.push(mount.to_path_buf());
    };
    let create = |name: &str| {
        let image = dir.join(name);
        run(
            "hdiutil",
            &[
                &os("create"),
                &os("-quiet"),
                &os("-size"),
                &os("20m"),
                &os("-fs"),
                &os("HFS+"),
                &os("-volname"),
                &os("probe"),
                image.as_os_str(),
            ],
        );
        image
    };
    let mut report = Vec::new();
    // Writable: before any Trash is made there, then the first move, then the question again.
    let image = create("probe-writable.dmg");
    let mount = dir.join("mount-writable");
    attach(&mut attached, &image, &mount, false);
    let (a, b) = (mount.join("first.tmp"), mount.join("second.tmp"));
    std::fs::write(&a, b"x").unwrap();
    std::fs::write(&b, b"x").unwrap();
    let asked_before = (has_trash_for(&a), has_trash_creating(&a), super::asks_for_trash(&a, false));
    let ids = ident(&a);
    let moved = move_to_trash(&a);
    let after = moved.as_ref().ok().and_then(|to| ident(to));
    report.push(format!(
        "writable, first file: asked_before(has_trash_for, create:YES, create:NO alone)={asked_before:?} moved={moved:?} source_there={} identity_kept={}",
        a.exists(),
        ids.is_some() && ids == after
    ));
    report.push(format!("writable, second file: has_trash_for={}", has_trash_for(&b)));
    // Read-only: a file put on the image, which is detached and attached again read-only.
    let image = create("probe-readonly.dmg");
    let mount_rw = dir.join("mount-rw");
    attach(&mut attached, &image, &mount_rw, false);
    std::fs::write(mount_rw.join("kept.tmp"), b"x").unwrap();
    run("hdiutil", &[&os("detach"), &os("-quiet"), mount_rw.as_os_str()]);
    attached.0.clear();
    attached.0.push(mount.clone());
    let mount_ro = dir.join("mount-ro");
    attach(&mut attached, &image, &mount_ro, true);
    let kept = mount_ro.join("kept.tmp");
    let asked = (has_trash_for(&kept), has_trash_creating(&kept), super::asks_for_trash(&kept, false));
    let moved = move_to_trash(&kept);
    report.push(format!(
        "read-only: asked(has_trash_for, create:YES, create:NO alone)={asked:?} moved={moved:?} source_still_there={}",
        kept.exists()
    ));
    println!("trash probe, HFS+ images, mount folders of its own:\n{}", report.join("\n"));
}
