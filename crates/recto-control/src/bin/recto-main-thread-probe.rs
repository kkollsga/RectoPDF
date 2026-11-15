//! For a test only, never in the app (`app/build.sh` copies the app's programs by name, and this isn't one): on this process's main thread, as the app's UI thread is, asks the indexer for the Find key, then asks again from another thread, and says what each ask took. The indexer's rule is the same whatever its plan, so a plan of a key in memory stands for the app's: the main thread's ask must take nothing (the plan's loader, the Keychain's in the app, never runs there). `tests/main_thread.rs` runs it.

use recto_control::find_keys::Plan;
use recto_control::indexing::Indexer;

fn main() {
    let root = std::env::temp_dir().join(format!("recto-main-thread-probe-{}", std::process::id()));
    let indexer = Indexer::new(&root, Plan::Memory);
    let on_main = recto_platform::thread::is_main_thread();
    let main_got = indexer.find_key().is_some();
    let main_took = indexer.find_key_report()["source"].clone();
    let elsewhere = indexer.clone();
    let other_got = std::thread::Builder::new()
        .name("recto-main-thread-probe".into())
        .spawn(move || elsewhere.find_key().is_some())
        .is_ok_and(|other| other.join().unwrap_or(false));
    let other_took = indexer.find_key_report()["source"].clone();
    println!("main={on_main} main_got={main_got} main_took={main_took} other_got={other_got} other_took={other_took}");
}
