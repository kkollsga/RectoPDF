//! The idle-test evidence policy runs in process; it needs no app launch or sampler process.

#[allow(dead_code)]
#[path = "app/awake.rs"]
mod awake;

#[allow(dead_code)]
#[path = "common/native_sample.rs"]
mod native_sample;

/// Failure: successful idle checks start a diagnostic and expire the quiet app, or lazy diagnostics suppress a real idle failure or its cause.
#[test]
fn successful_rest_skips_sampling_and_a_failure_keeps_its_evidence() {
    awake::assert_rest(&[(0, 0)], &[(4, 2), (0, 0), (1, 0)], || panic!("a successful rest started a diagnostic"));
    let failure = std::panic::catch_unwind(|| awake::assert_rest(&[(0, 0)], &[(81, 0)], || "fixture evidence".into())).unwrap_err();
    let text = failure.downcast_ref::<String>().expect("rest failure text");
    assert!(text.contains("it wakes at rest") && text.contains("fixture evidence"), "{text}");
    let diagnostic_failure = std::panic::catch_unwind(|| awake::assert_rest(&[(9, 0)], &[], || panic!("fixture diagnostic failure"))).unwrap_err();
    let text = diagnostic_failure.downcast_ref::<String>().expect("rest failure text");
    assert!(text.contains("it never came to rest") && text.contains("fixture diagnostic failure"), "{text}");
}

/// Failure: a bounded sampler refusal itself fails evidence validation or omits why the native sample could not be collected.
#[test]
fn a_bounded_sampler_failure_is_valid_failure_evidence() {
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    let message = native_sample::from_capture(Ok((
        true,
        std::process::Output {
            status,
            stdout: vec![],
            stderr: vec![],
        },
    )));
    assert!(message.starts_with("sample failed") && message.contains("byte/time bound"), "{message}");
    let missing = native_sample::from_capture(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "fixture timed out")));
    assert!(missing.starts_with("sample failed") && missing.contains("fixture timed out"), "{missing}");
    assert!(awake::stack_summary(&message).starts_with("sample failed"));
}
