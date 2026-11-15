//! **What a released copy's own updater makes of a newer release**, built by `tools/release/verdict.sh` from that release's own source (the
//! tag's `crates/recto-update`, at its own `Cargo.lock`), never from today's. A released binary has no command that reads a feed from a
//! file, and in a release build none can be pointed at another server, so this is its code with the network taken out: what
//! `stage::prepare` does after the download, step for step, on a zip already here and a folder its own `check-zip` unpacked.
//!
//!   release-verdict <latest.json> <zip> <unpacked folder> <running version> <macOS version>
//!
//! One JSON line per step, the last `{"verdict":"takes"|"refuses",...}`. Exit 0 when it takes it, 1 when it refuses, 2 on wrong use.
//! Written against v0.1.0's API (`rules::parse_latest`, `Latest::on_offer`, `fetch::MAX_ZIP_BYTES`, `stage::verify_app`). A later release whose
//! API differs needs this file changed to match, which the build says at once.

use recto_update::source::Trust;
use recto_update::{APP_BUNDLE, BUNDLE_ID, TEAM_ID, fetch, rules, stage};
use sha2::{Digest as _, Sha256};
use std::io::Read as _;
use std::path::Path;

fn say(step: &str, ok: bool, detail: &str) {
    println!("{}", serde_json::json!({ "step": step, "ok": ok, "detail": detail }));
}

fn refuse(step: &str, detail: &str) -> ! {
    say(step, false, detail);
    println!("{}", serde_json::json!({ "verdict": "refuses", "at": step, "detail": detail }));
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [feed, zip, unpacked, running, system] = args.as_slice() else {
        eprintln!("usage: release-verdict <latest.json> <zip> <unpacked folder> <running version> <macOS version>");
        std::process::exit(2);
    };
    let Ok(data) = std::fs::read(feed) else { refuse("feed", "latest.json can't be read") };
    // What the updater keeps of the feed, and only if every field is what a release writes.
    let Some(latest) = rules::parse_latest(&data) else { refuse("feed", "parse_latest refuses latest.json") };
    say("feed", true, &format!("version {} sha256 {} minimum macOS {:?}", latest.version, latest.sha256, latest.minimum_macos));
    if running == &latest.version {
        say("offer", true, &format!("not asked: the copy asking is {running} itself"));
    } else if latest.on_offer(running, system) {
        say("offer", true, &format!("offered to {running} on macOS {system}"));
    } else {
        refuse("offer", &format!("not offered to {running} on macOS {system}: not newer, or this Mac is too old for it"));
    }
    // The download: no larger than the most it takes, and the very file the feed describes.
    let Ok(meta) = std::fs::metadata(zip) else { refuse("zip", "the zip can't be read") };
    if meta.len() > fetch::MAX_ZIP_BYTES {
        refuse("zip", &format!("{} bytes, over the {} a download may be", meta.len(), fetch::MAX_ZIP_BYTES));
    }
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(zip).unwrap_or_else(|_| refuse("zip", "the zip can't be opened"));
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => hasher.update(&buffer[..count]),
            Err(_) => refuse("zip", "the zip can't be read through"),
        }
    }
    let got: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if got != latest.sha256 {
        refuse("checksum", &format!("the zip is {got}, the feed says {}", latest.sha256));
    }
    say("checksum", true, &got);
    // The app its own reader unpacked: this app, this version, signed whole, by the team, and taken by Gatekeeper.
    let app = Path::new(unpacked).join(APP_BUNDLE);
    match stage::verify_app(&app, Trust::DeveloperId { team: TEAM_ID }, &latest.version, BUNDLE_ID) {
        Ok(cdhash) => say("verify", true, &format!("cdhash {cdhash}")),
        Err(error) => refuse("verify", &format!("{}: {}", error.code, error.why)),
    }
    println!("{}", serde_json::json!({ "verdict": "takes", "version": latest.version }));
}
