//! The probe that measured what the volume UUID does (`--ignored`, makes disk images with `hdiutil` in a temp folder and detaches every one it
//! attached, in a drop guard; it touches no other disk), and the checks that run everywhere.

use super::*;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;

/// Failure: a path that can't be looked at, or a non-existent one, is given an identity and joins versions it should not.
#[test]
fn a_missing_path_has_no_identity() {
    assert_eq!(volume_identity(Path::new("/nonexistent/recto/volume")), None);
}

/// Failure: the call is wrong (a bad struct, a wrong flag) and the boot volume reads as having none, so every removal refuses; or it
/// changes between two looks at the same file.
#[test]
fn the_same_folder_gives_the_same_answer_twice() {
    let here = std::env::temp_dir();
    assert_eq!(volume_identity(&here), volume_identity(&here));
    if let Some(uuid) = volume_identity(&here) {
        assert_ne!(uuid, [0; 16]);
    }
}

/// Every image this probe attached, detached on drop (a failed assertion or a panic included), by the mount folder it made. `-force` only
/// after a plain detach is refused, and only of its own mount points.
struct Attached(Vec<PathBuf>);

impl Drop for Attached {
    fn drop(&mut self) {
        for mount in self.0.drain(..).rev() {
            let plain = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
            if !plain.is_ok_and(|s| s.success()) {
                let _ = Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mount).status();
            }
        }
    }
}

fn run(program: &str, args: &[&str]) -> String {
    let out = Command::new(program).args(args).output().expect("run");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{program} {args:?} failed: {text}");
    text
}

fn hex(uuid: Option<[u8; 16]>) -> String {
    uuid.map_or("none".into(), |u| u.iter().map(|b| format!("{b:02x}")).collect())
}

fn attach(image: &Path, mount: &Path, attached: &mut Attached) -> Result<(), String> {
    std::fs::create_dir_all(mount).unwrap();
    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-noautoopen", "-mountpoint"])
        .arg(mount)
        .arg(image)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        attached.0.push(mount.to_path_buf());
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn detach(mount: &Path, attached: &mut Attached) {
    run("hdiutil", &["detach", "-quiet", mount.to_str().unwrap()]);
    attached.0.retain(|m| m != mount);
}

fn report(step: &str, mount: &Path) {
    let file = mount.join("probe.txt");
    if !file.exists() {
        std::fs::write(&file, b"probe").unwrap();
    }
    let meta = std::fs::metadata(&file).unwrap();
    let seen = look(&file).expect("statfs");
    let by_mount = look(Path::new(std::str::from_utf8(&seen.mount_point).unwrap())).and_then(|s| s.uuid);
    println!(
        "{step:<28} st_dev={:<10} fsid={:?} type={:<6} uuid(file)={} uuid(mnt)={} identity={} ino={} size={} mtime_ns={}",
        meta.dev(),
        seen.fsid,
        String::from_utf8_lossy(&seen.fs_type),
        hex(seen.uuid),
        hex(by_mount),
        hex(volume_identity(&file)),
        meta.ino(),
        meta.len(),
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
    );
}

/// The measurement of design §4 / brief step 0. Not a regression test: it prints what this macOS did.
#[test]
#[ignore = "makes and attaches disk images with hdiutil; run by hand: cargo test -p recto-platform volume_probe -- --ignored --nocapture"]
fn volume_probe() {
    let dir = recto_testpdf::TempDir::new("recto-volume-probe");
    let mut attached = Attached(Vec::new());
    println!("macOS {}", run("sw_vers", &["-productVersion"]).trim());
    for fs in ["APFS", "HFS+", "ExFAT", "MS-DOS"] {
        let tag = fs.replace('+', "plus");
        let image = dir.path().join(format!("{tag}-a.dmg"));
        let copy = dir.path().join(format!("{tag}-b.dmg"));
        run(
            "hdiutil",
            &[
                "create",
                "-quiet",
                "-size",
                "20m",
                "-fs",
                fs,
                "-volname",
                &format!("RPA{tag}"),
                image.to_str().unwrap(),
            ],
        );
        println!("--- {fs}");
        let a = dir.path().join(format!("mnt-{tag}-a"));
        attach(&image, &a, &mut attached).expect("attach A");
        report("A attached", &a);
        detach(&a, &mut attached);
        attach(&image, &a, &mut attached).expect("attach A again");
        report("A re-attached", &a);
        detach(&a, &mut attached);
        std::fs::copy(&image, &copy).unwrap();
        let b = dir.path().join(format!("mnt-{tag}-b"));
        attach(&copy, &b, &mut attached).expect("attach B alone");
        report("B (cp of A) alone", &b);
        detach(&b, &mut attached);
        attach(&image, &a, &mut attached).expect("attach A");
        match attach(&copy, &b, &mut attached) {
            Ok(()) => {
                report("A with B: A", &a);
                report("A with B: B", &b);
            }
            Err(e) => println!("A and B together: REFUSED: {}", e.trim()),
        }
        detach(&b, &mut attached);
        detach(&a, &mut attached);
    }
}
