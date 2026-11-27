//! **A start or an open the Mac had no descriptors or memory for, for a moment.** `EMFILE`, `ENFILE`, `EAGAIN` and `ENOMEM` from opening the file, and those or `ENOSPC` from the shared memory a renderer needs (`shm_open`, `ftruncate`, `mmap`), say nothing of the file: the Mac was out of something and will have it again. They are [`RenderFailure::OutOfResources`], which [`RenderFailure::is_busy`] counts, so whoever asked asks again by the rule it has for a busy Mac, and the document isn't failed. Any other error stays [`RenderFailure::Unavailable`].

use super::RenderFailure;
use recto_platform::isolate::SharedMemory;
use std::io;

/// Whether `error` is the Mac being out of descriptors or memory. `enospc` is for the shared memory's errors only: for a file's open it says the disk is full, which isn't this.
/// macOS's errno values, named here so this crate takes no dependency for them (the app only runs there).
#[cfg(target_os = "macos")]
mod errno {
    pub const ENOMEM: i32 = 12;
    pub const ENFILE: i32 = 23;
    pub const EMFILE: i32 = 24;
    pub const ENOSPC: i32 = 28;
    pub const EAGAIN: i32 = 35;
}

#[cfg(target_os = "macos")]
fn is_out_of_resources(error: &io::Error, enospc: bool) -> bool {
    match error.raw_os_error() {
        Some(code) => code == errno::EMFILE || code == errno::ENFILE || code == errno::EAGAIN || code == errno::ENOMEM || (enospc && code == errno::ENOSPC),
        None => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn is_out_of_resources(_error: &io::Error, _enospc: bool) -> bool {
    false
}

/// What a failed open of the document's file is: out of resources for a moment, or `Unavailable`. Says the errno, and nothing of the path.
pub(super) fn of_open(error: &io::Error) -> RenderFailure {
    of(error, false, "open")
}

/// What a failed creation of the renderer's shared memory is.
pub(super) fn of_shared_memory(error: &io::Error) -> RenderFailure {
    of(error, true, "shared memory")
}

fn of(error: &io::Error, enospc: bool, step: &str) -> RenderFailure {
    if is_out_of_resources(error, enospc) {
        eprintln!("recto-render-service: out of resources at {step}, errno {}", error.raw_os_error().unwrap_or(0));
        RenderFailure::OutOfResources
    } else {
        RenderFailure::Unavailable
    }
}

#[cfg(any(test, feature = "test-seams"))]
thread_local! {
    static FAIL_OPEN: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static FAIL_SHARED_MEMORY: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Test seam: the next open of a document's file on this thread fails with this errno, once.
#[cfg(any(test, feature = "test-seams"))]
pub fn fail_next_open_with(errno: i32) {
    FAIL_OPEN.with(|cell| cell.set(errno));
}

/// Test seam: the next creation of a renderer's shared memory on this thread fails with this errno, once.
#[cfg(any(test, feature = "test-seams"))]
pub fn fail_next_shared_memory_with(errno: i32) {
    FAIL_SHARED_MEMORY.with(|cell| cell.set(errno));
}

/// Opens the document's file through the wall: what the open said, as the failure it is.
pub(super) fn open_document(path: &std::path::Path) -> Result<std::fs::File, RenderFailure> {
    use recto_platform::document_file::{self, Refused};
    if let Some(failure) = injected_open() {
        return Err(failure);
    }
    document_file::open(path).map_err(|refused| match refused {
        Refused::Look(error) | Refused::Open(error) if error.kind() == io::ErrorKind::NotFound => RenderFailure::NotFound,
        Refused::NotADocument => RenderFailure::NotPdf,
        Refused::Look(error) | Refused::Open(error) => of_open(&error),
    })
}

/// The injected failure for an open, if one is set.
pub(super) fn injected_open() -> Option<RenderFailure> {
    #[cfg(any(test, feature = "test-seams"))]
    {
        let code = FAIL_OPEN.with(|cell| cell.replace(0));
        if code != 0 {
            return Some(of_open(&io::Error::from_raw_os_error(code)));
        }
    }
    None
}

/// The renderer's shared memory, or why it couldn't be had.
pub(super) fn create_shared_memory(len: usize) -> Result<SharedMemory, RenderFailure> {
    #[cfg(any(test, feature = "test-seams"))]
    {
        let code = FAIL_SHARED_MEMORY.with(|cell| cell.replace(0));
        if code != 0 {
            return Err(of_shared_memory(&io::Error::from_raw_os_error(code)));
        }
    }
    SharedMemory::create(len).map_err(|error| of_shared_memory(&error))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::host::{DocRenderer, Policy, ServiceConfig};
    use std::path::Path;

    const EACCES: i32 = 13;

    /// Failure: an open that ran out of descriptors failed the whole document ("The page renderer couldn't be started") for a Mac that was only short for a moment. It's `OutOfResources`, which counts as busy, and an error that is about the file (a refusal) stays `Unavailable` and is not busy.
    #[test]
    fn running_out_of_descriptors_is_busy_and_a_refusal_is_not() {
        for errno in [errno::EMFILE, errno::ENFILE, errno::EAGAIN, errno::ENOMEM] {
            fail_next_open_with(errno);
            let failure = DocRenderer::open_file(Path::new("/dev/null/anything")).unwrap_err();
            assert_eq!(failure, RenderFailure::OutOfResources, "errno {errno}");
            assert!(failure.is_busy());
        }
        // A disk that is full is not this for a file's open.
        fail_next_open_with(errno::ENOSPC);
        assert_eq!(DocRenderer::open_file(Path::new("/dev/null/anything")).unwrap_err(), RenderFailure::Unavailable);
        fail_next_open_with(EACCES);
        let failure = DocRenderer::open_file(Path::new("/dev/null/anything")).unwrap_err();
        assert_eq!(failure, RenderFailure::Unavailable);
        assert!(!failure.is_busy());
    }

    /// Failure: the shared memory a renderer needs not being had (`shm_open`, `ftruncate` or `mmap` out of descriptors, memory or space) failed the document for good. It's `OutOfResources`; a refusal is `Unavailable`.
    #[test]
    fn shared_memory_out_of_space_is_busy_and_a_refusal_is_not() {
        use std::os::fd::AsFd;
        let file = std::fs::File::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
        let attempt = |errno: i32| {
            fail_next_shared_memory_with(errno);
            DocRenderer::with_file(
                file.try_clone().unwrap(),
                Path::new("/dev/null"),
                None,
                ServiceConfig {
                    worker_exe: "x".into(),
                    pdfium_lib: "y".into(),
                    heavy_slot: Default::default(),
                    extra_env: Vec::new(),
                    replace_args: None,
                },
                Policy::default(),
            )
            .err()
        };
        for errno in [errno::EMFILE, errno::ENFILE, errno::EAGAIN, errno::ENOMEM, errno::ENOSPC] {
            assert_eq!(attempt(errno), Some(RenderFailure::OutOfResources), "errno {errno}");
        }
        let failure = attempt(EACCES).unwrap();
        assert_eq!(failure, RenderFailure::Unavailable);
        assert!(!failure.is_busy());
    }
}
