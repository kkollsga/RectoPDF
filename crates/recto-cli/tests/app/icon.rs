//! The app's icon in the built bundle. Its artwork and sizes come from `app/icon/` (`icons.swift`, built by `app/icon/build.sh`); these check
//! what `app/build.sh` puts in the bundle, which is what Finder, the Dock and the update zip see.

use super::app_path;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The built bundle's `Contents`, unless the app isn't built (then the test skips, or fails where a run needs the app).
fn contents() -> Option<PathBuf> {
    Some(app_path()?.parent()?.parent()?.to_path_buf())
}

fn info_plist(contents: &Path) -> Value {
    let out = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(contents.join("Info.plist"))
        .output()
        .expect("plutil runs");
    assert!(
        out.status.success(),
        "the bundle's Info.plist doesn't read: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("plutil writes JSON")
}

/// One entry of an `.icns` file: its four-letter type and its payload.
type Entry<'a> = ([u8; 4], &'a [u8]);

/// What a bundle's icon needs at one size: what it is, its pixels, and the `.icns` types that can hold it.
type Needed = (&'static str, u32, &'static [&'static [u8; 4]]);

/// An `.icns` file's entries, or why it isn't one.
fn icns_entries(bytes: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
    if bytes.get(..4) != Some(b"icns".as_slice()) || word(4) != Some(bytes.len()) {
        return Err("no `icns` header with the file's length".into());
    }
    let mut entries = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let len = word(at + 4).ok_or("an entry cut short")?;
        if len < 8 || at + len > bytes.len() {
            return Err(format!("an entry at {at} with a length of {len} doesn't fit"));
        }
        entries.push((bytes[at..at + 4].try_into().unwrap(), &bytes[at + 8..at + len]));
        at += len;
    }
    Ok(entries)
}

/// A PNG's width, if the payload is one.
fn png_width(payload: &[u8]) -> Option<u32> {
    if payload.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice()) {
        return None;
    }
    Some(u32::from_be_bytes(payload.get(16..20)?.try_into().ok()?))
}

/// Failure: the app ships with the generic app icon. The bundle lacks `Resources/AppIcon.icns`, Info.plist doesn't name it in
/// `CFBundleIconFile`, or Info.plist names an Icon Composer icon (`CFBundleIconName`) with no `Assets.car` to hold it, which macOS 26
/// would show as blank.
#[test]
fn the_built_app_has_its_icon() {
    let Some(contents) = contents() else { return };
    let info = info_plist(&contents);
    let file = info["CFBundleIconFile"].as_str().expect("Info.plist has no CFBundleIconFile");
    let name = if file.ends_with(".icns") { file.to_string() } else { format!("{file}.icns") };
    assert!(
        contents.join("Resources").join(&name).is_file(),
        "Info.plist names {name}, which isn't in Contents/Resources"
    );
    if let Some(catalog_name) = info.get("CFBundleIconName").and_then(Value::as_str) {
        assert!(!catalog_name.is_empty(), "CFBundleIconName is empty");
        assert!(
            contents.join("Resources/Assets.car").is_file(),
            "Info.plist names the icon {catalog_name} in an asset catalog, and there's no Contents/Resources/Assets.car"
        );
    }
}

/// Failure: the icon file misses a size macOS asks for, so Finder scales another one and a list view or the Dock shows it blurred, or
/// an entry isn't the size its type says. A bundle needs 16, 32, 128, 256 and 512 points at 1x and 2x: ten images.
#[test]
fn the_icon_holds_every_size_a_mac_asks_for() {
    let Some(contents) = contents() else { return };
    let file = info_plist(&contents)["CFBundleIconFile"]
        .as_str()
        .map(str::to_owned)
        .expect("Info.plist has no CFBundleIconFile");
    let file = if file.ends_with(".icns") { file } else { format!("{file}.icns") };
    let bytes = std::fs::read(contents.join("Resources").join(&file)).expect("the icon file reads");
    let entries = icns_entries(&bytes).unwrap_or_else(|why| panic!("{file} isn't an icns file: {why}"));
    // 16 and 32 at 1x may be ARGB rather than PNG.
    let needed: [Needed; 10] = [
        ("16 pt", 16, &[b"icp4", b"ic04"]),
        ("16 pt @2x", 32, &[b"ic11"]),
        ("32 pt", 32, &[b"icp5", b"ic05"]),
        ("32 pt @2x", 64, &[b"ic12"]),
        ("128 pt", 128, &[b"ic07"]),
        ("128 pt @2x", 256, &[b"ic13"]),
        ("256 pt", 256, &[b"ic08"]),
        ("256 pt @2x", 512, &[b"ic14"]),
        ("512 pt", 512, &[b"ic09"]),
        ("512 pt @2x", 1024, &[b"ic10"]),
    ];
    for (what, pixels, types) in needed {
        let Some((kind, payload)) = entries.iter().find(|(kind, _)| types.contains(&kind)) else {
            let have: Vec<String> = entries.iter().map(|(kind, _)| String::from_utf8_lossy(kind).into_owned()).collect();
            panic!("{file} has no {what} image ({pixels} px); it has {have:?}");
        };
        if let Some(width) = png_width(payload) {
            assert_eq!(width, pixels, "{file}'s {what} image ({}) is {width} px wide", String::from_utf8_lossy(kind));
        }
    }
}
