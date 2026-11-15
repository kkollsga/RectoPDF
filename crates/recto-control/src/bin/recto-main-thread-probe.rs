//! For a test only, never in the app (`app/build.sh` copies the app's programs by name, and this isn't one): on this process's main thread, as the app's UI thread is, asks the indexer for the Find key, then asks again from another thread, and says which asks reached the key's loader. `tests/main_thread.rs` runs it.

use recto_control::find_keys::Plan;
use recto_control::indexing::Indexer;
use recto_platform::find_key::FindKey;
use std::sync::atomic::{AtomicBool, Ordering};

static REACHED: AtomicBool = AtomicBool::new(false);

/// Stands in for the Keychain: notes that it was reached.
fn loader() -> Option<(FindKey, Option<&'static str>)> {
    REACHED.store(true, Ordering::SeqCst);
    FindKey::in_memory().ok().map(|key| (key, None))
}

fn main() {
    let root = std::env::temp_dir().join(format!("recto-main-thread-probe-{}", std::process::id()));
    let indexer = Indexer::new(&root, Plan::AppKeychain(loader));
    let on_main = recto_platform::thread::is_main_thread();
    let main_got = indexer.find_key().is_some();
    let main_reached = REACHED.load(Ordering::SeqCst);
    let elsewhere = indexer.clone();
    let other_got = std::thread::Builder::new()
        .name("recto-main-thread-probe".into())
        .spawn(move || elsewhere.find_key().is_some())
        .is_ok_and(|other| other.join().unwrap_or(false));
    let other_reached = REACHED.load(Ordering::SeqCst);
    println!("main={on_main} main_got={main_got} main_reached={main_reached} other_got={other_got} other_reached={other_reached}");
}
