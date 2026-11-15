//! `recto-keychain-probe`: the app's own way to its Find key (`recto_platform::find_key::load_for_the_app`'s code, through
//! `find_key::probe`), run by the release preflight in a bundle signed as the release is, so what the preflight measures is the code
//! that ships (the security read of 2026-11-15, F3): the same dictionaries, the same process-wide switch and the same release check,
//! under a probe's service name so it never meets the person's real item. Built only with the `keychain-probe` feature, by
//! `tools/release/dp-keychain-probe.sh`, and never put in the app.
//!
//!   recto-keychain-probe <load|remove> <team> <bundle id>
//!
//! `load` prints `source=<where the key came from> why=<the refusal's code, or ->` and `key=<16 hex of a hash of the key>`, so a second
//! launch can be seen to read the same key; never the key. `remove` prints `status=<OSStatus>`.

#[cfg(feature = "keychain-probe")]
fn main() {
    use recto_platform::find_key::{App, probe};
    use std::hash::{DefaultHasher, Hash, Hasher};
    const SERVICE: &str = "io.github.kkollsga.RectoPDF.find-text-probe";
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(team), Some(bundle)) = (args.get(1), args.get(2), args.get(3)) else {
        eprintln!("usage: recto-keychain-probe <load|remove> <team> <bundle id>");
        std::process::exit(2);
    };
    // The probe's names live as long as the process, as the app's constants do.
    let app = App {
        team: Box::leak(team.clone().into_boxed_str()),
        bundle: Box::leak(bundle.clone().into_boxed_str()),
    };
    match mode.as_str() {
        "load" => match probe::load(app, SERVICE) {
            Some((key, why)) => {
                let mut hash = DefaultHasher::new();
                key.bytes().hash(&mut hash);
                println!("source={} why={} key={:016x}", key.source().code(), why.unwrap_or("-"), hash.finish());
            }
            None => println!("source=none why=no_randomness"),
        },
        "remove" => println!("status={}", probe::remove(app, SERVICE)),
        _ => {
            eprintln!("usage: recto-keychain-probe <load|remove> <team> <bundle id>");
            std::process::exit(2);
        }
    }
}

#[cfg(not(feature = "keychain-probe"))]
fn main() {}
