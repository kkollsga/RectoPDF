//! Windows comes later: it gets composition surfaces of its own. These keep everything above compiling, and fail plainly if anything tries to use them.

use std::io;
use std::time::Duration;

fn unsupported<T>() -> io::Result<T> {
    Err(io::ErrorKind::Unsupported.into())
}

#[derive(Clone, Debug)]
pub struct PixelSurface(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purge {
    Kept,
    Emptied,
}

impl PixelSurface {
    pub fn create(_width: u32, _height: u32) -> io::Result<Self> {
        unsupported()
    }
    pub fn id(&self) -> u32 {
        0
    }
    /// # Safety
    /// There's nothing to take back here: this never returns one.
    pub unsafe fn from_host_reference(_reference: u64) -> Option<Self> {
        None
    }
    pub fn width(&self) -> u32 {
        0
    }
    pub fn height(&self) -> u32 {
        0
    }
    pub fn stride(&self) -> u32 {
        0
    }
    pub fn len(&self) -> usize {
        0
    }
    pub fn is_empty(&self) -> bool {
        true
    }
    pub fn retained_for_host(&self) -> u64 {
        0
    }
    pub fn with_pixels<R>(&self, _step: impl FnOnce(&mut [u8]) -> R) -> io::Result<R> {
        unsupported()
    }
    pub fn release_for_host(_reference: u64) {}
    #[doc(hidden)]
    pub fn reference_count(&self) -> isize {
        0
    }
    pub fn set_discardable(&self) {}
    #[doc(hidden)]
    pub fn empty_now(&self) {}
    pub fn reclaim(&self) -> Purge {
        Purge::Emptied
    }
    pub fn was_emptied(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct LinkInvite(());

impl LinkInvite {
    pub fn new() -> io::Result<Self> {
        unsupported()
    }
    pub fn accept(self, _wait: Duration) -> Option<SurfaceSender> {
        None
    }
}

#[derive(Debug)]
pub struct SurfaceSender(());

impl SurfaceSender {
    #[doc(hidden)]
    pub fn dead() -> Self {
        Self(())
    }
    pub fn send(&self, _surface: &PixelSurface) -> io::Result<()> {
        unsupported()
    }
}

#[derive(Debug)]
pub struct SurfaceReceiver(());

impl SurfaceReceiver {
    pub fn from_invite() -> Option<Self> {
        None
    }
    pub fn next(&self, _wait: Duration) -> Option<PixelSurface> {
        None
    }
}
