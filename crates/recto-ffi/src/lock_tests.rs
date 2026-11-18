use std::sync::Mutex;

/// Failure: sharing the core's lock helper makes a bridge call panic after an earlier owner panicked, or discards the value that owner wrote.
#[test]
fn a_poisoned_bridge_lock_retains_its_written_value() {
    let value = Mutex::new(0);
    let poisoned = std::panic::catch_unwind(|| {
        let mut held = value.lock().expect("not poisoned yet");
        *held = 41;
        panic!("poison the held lock");
    });
    assert!(poisoned.is_err());
    assert!(value.is_poisoned());
    let mut held = crate::lock(&value);
    assert_eq!(*held, 41);
    *held += 1;
    drop(held);
    assert_eq!(*crate::lock(&value), 42);
}
