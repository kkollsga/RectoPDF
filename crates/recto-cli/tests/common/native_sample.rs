//! One bounded sampler for failure evidence. There is no output file or inherited writer outside the collector's limits.

use std::io;
use std::process::{Command, Output};
use std::time::Duration;

const SAMPLE_CAP: usize = 1024 * 1024;

/// The sample's command line. The tests run under `taskpolicy -c background` (CI's limits, `tools/check-like-ci.sh`) and a child inherits that clamp: at load 4.6 a plain 3 s `sample` took 8 s and a clamped one 12 s against a 18 s bound, so under real load the capture ran past its bound and the evidence said only "capture exceeded its byte/time bound" (2026-11-28, three failures). `taskpolicy -c utility` lifts the sampler to the utility clamp (`ps` priority 4 under background, 20 with this, 31 unclamped); the app under test keeps its clamp, since only this child is changed.
pub(super) fn sample_command(pid: u32, seconds: u32) -> io::Result<Command> {
    if pid <= 1 || !(1..=10).contains(&seconds) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid sample target or duration"));
    }
    let mut command = Command::new("/usr/sbin/taskpolicy");
    command
        .args(["-c", "utility", "/usr/bin/sample"])
        .arg(pid.to_string())
        .arg(seconds.to_string())
        .args(["-mayDie", "-file", "/dev/stdout"]);
    Ok(command)
}

pub fn sample(pid: u32, seconds: u32, timeout: Duration) -> String {
    #[cfg(target_os = "macos")]
    {
        from_capture(sample_command(pid, seconds).and_then(|mut command| recto_platform::test_command::output_with_cap(&mut command, timeout, SAMPLE_CAP)))
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
