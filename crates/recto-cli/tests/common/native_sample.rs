//! One bounded sampler for failure evidence. There is no output file or inherited writer outside the collector's limits.

use std::io;
use std::process::Output;
use std::time::Duration;

const SAMPLE_CAP: usize = 1024 * 1024;

pub fn sample(pid: u32, seconds: u32, timeout: Duration) -> String {
    #[cfg(target_os = "macos")]
    {
        from_capture(recto_platform::test_command::sample_process(pid, seconds, timeout, SAMPLE_CAP))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, seconds, timeout, SAMPLE_CAP);
        "sample failed: native sampling is available on macOS only\n".into()
    }
}

pub(super) fn from_capture(result: io::Result<(bool, Output)>) -> String {
    match result {
        Ok((false, output)) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        Ok((exceeded, output)) => format!(
            "sample failed: {} ({}); {}\n{}",
            if exceeded {
                "capture exceeded its byte/time bound"
            } else {
                "the native tool failed"
            },
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ),
        Err(error) => format!("sample failed: {error}\n"),
    }
}
