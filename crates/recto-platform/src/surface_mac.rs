//! Pixel memory the compositor can show directly: an IOSurface, and the way one reaches a worker without a copy.
//!
//! **How a tile gets to the screen.** The host creates the surface, so it's charged to the host and survives the worker. A socket pair can't carry an IOSurface, because it crosses processes as a Mach port and
//! a socket carries descriptors. So each worker is started with a send right to a port of the host's in one of its exception-port slots, set by `posix_spawn` and never otherwise used, a slot that doesn't touch
//! the bootstrap port or anything the system libraries look at. The worker reads it, sends back a send right to a port of its own, and clears the slot. From then on the host sends each surface to that port.
//! There's no name in any registry, and nothing else can reach it. Measured on an M4 (64 surfaces of 512 x 512): the host's footprint grows by 1 MiB a tile, the worker's by nothing, and creating a surface,
//! sending it, looking it up, filling it and hearing back takes 0.13 ms.
//!
//! Everything here is a seam: when the renderer moves into an XPC service, `IOSurfaceCreateXPCObject` replaces the port and nothing above this file changes.

use std::ffi::{c_char, c_int, c_void};
use std::io;
use std::ptr::NonNull;
use std::time::Duration;

type CfRef = *const c_void;
type IoSurfaceRef = *mut c_void;
type MachPort = u32;
type KernReturn = c_int;

#[repr(C)]
struct CfDictionaryKeyCallBacks([*const c_void; 6]);
#[repr(C)]
struct CfDictionaryValueCallBacks([*const c_void; 5]);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: CfDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CfDictionaryValueCallBacks;
    fn CFNumberCreate(allocator: CfRef, kind: isize, value: *const c_void) -> CfRef;
    fn CFDictionaryCreate(allocator: CfRef, keys: *const CfRef, values: *const CfRef, count: isize, key_callbacks: *const CfDictionaryKeyCallBacks, value_callbacks: *const CfDictionaryValueCallBacks) -> CfRef;
    fn CFStringCreateWithCString(allocator: CfRef, text: *const c_char, encoding: u32) -> CfRef;
    fn CFRelease(value: CfRef);
    fn CFRetain(value: CfRef) -> CfRef;
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    static kIOSurfaceWidth: CfRef;
    static kIOSurfaceHeight: CfRef;
    static kIOSurfaceBytesPerElement: CfRef;
    static kIOSurfacePixelFormat: CfRef;
    static kIOSurfaceColorSpace: CfRef;
    fn IOSurfaceCreate(properties: CfRef) -> IoSurfaceRef;
    fn IOSurfaceGetAllocSize(surface: IoSurfaceRef) -> usize;
    fn IOSurfaceGetBytesPerRow(surface: IoSurfaceRef) -> usize;
    fn IOSurfaceGetWidth(surface: IoSurfaceRef) -> usize;
    fn IOSurfaceGetHeight(surface: IoSurfaceRef) -> usize;
    fn IOSurfaceGetBaseAddress(surface: IoSurfaceRef) -> *mut c_void;
    fn IOSurfaceLock(surface: IoSurfaceRef, options: u32, seed: *mut u32) -> KernReturn;
    fn IOSurfaceUnlock(surface: IoSurfaceRef, options: u32, seed: *mut u32) -> KernReturn;
    fn IOSurfaceSetPurgeable(surface: IoSurfaceRef, new_state: u32, old_state: *mut u32) -> KernReturn;
    fn IOSurfaceSetValue(surface: IoSurfaceRef, key: CfRef, value: CfRef);
    fn IOSurfaceCreateMachPort(surface: IoSurfaceRef) -> MachPort;
    fn IOSurfaceLookupFromMachPort(port: MachPort) -> IoSurfaceRef;
}

const K_CF_NUMBER_INT32: isize = 3;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const PURGEABLE_NON_VOLATILE: u32 = 0;
const PURGEABLE_VOLATILE: u32 = 1;
const PURGEABLE_EMPTY: u32 = 2;
const PURGEABLE_KEEP_CURRENT: u32 = 3;

/// One BGRA surface. A shared count underneath: the compositor and a worker can each hold it too.
#[derive(Debug)]
pub struct PixelSurface {
    raw: NonNull<c_void>,
}

// SAFETY: an IOSurface is a thread-safe, reference-counted kernel object, and nothing here keeps per-thread state.
unsafe impl Send for PixelSurface {}
// SAFETY: as above. The only shared mutable thing is the memory, reached through `with_pixels`, which takes the surface's own lock.
unsafe impl Sync for PixelSurface {}

/// What became of a surface that was marked as discardable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purge {
    /// Its pixels are still there.
    Kept,
    /// The OS took them under pressure.
    Emptied,
}

impl PixelSurface {
    /// A new surface, tagged sRGB. Costs almost nothing until written: 16 to 40 µs, and no footprint.
    pub fn create(width: u32, height: u32) -> io::Result<Self> {
        let (Ok(w), Ok(h)) = (i32::try_from(width), i32::try_from(height)) else { return Err(io::ErrorKind::InvalidInput.into()) };
        if w <= 0 || h <= 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let bytes_per_element = 4i32;
        let format = PIXEL_FORMAT_BGRA as i32;
        // SAFETY: each `CFNumberCreate` reads the i32 it's pointed at, and `CFDictionaryCreate` retains what it's given, so everything made here is released here. The keys are the framework's own constants.
        let raw = unsafe {
            let values = [
                CFNumberCreate(std::ptr::null(), K_CF_NUMBER_INT32, (&raw const w).cast()),
                CFNumberCreate(std::ptr::null(), K_CF_NUMBER_INT32, (&raw const h).cast()),
                CFNumberCreate(std::ptr::null(), K_CF_NUMBER_INT32, (&raw const bytes_per_element).cast()),
                CFNumberCreate(std::ptr::null(), K_CF_NUMBER_INT32, (&raw const format).cast()),
            ];
            let keys = [kIOSurfaceWidth, kIOSurfaceHeight, kIOSurfaceBytesPerElement, kIOSurfacePixelFormat];
            let properties = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 4, &raw const kCFTypeDictionaryKeyCallBacks, &raw const kCFTypeDictionaryValueCallBacks);
            let raw = IOSurfaceCreate(properties);
            CFRelease(properties);
            for value in values {
                CFRelease(value);
            }
            raw
        };
        let raw = NonNull::new(raw).ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))?;
        let surface = Self { raw };
        // SAFETY: the surface is live, and the string is released after the surface has retained it.
        unsafe {
            let name = CFStringCreateWithCString(std::ptr::null(), c"kCGColorSpaceSRGB".as_ptr(), K_CF_STRING_ENCODING_UTF8);
            if !name.is_null() {
                IOSurfaceSetValue(surface.raw.as_ptr(), kIOSurfaceColorSpace, name);
                CFRelease(name);
            }
        }
        Ok(surface)
    }

    pub fn width(&self) -> u32 {
        // SAFETY: the surface is live.
        u32::try_from(unsafe { IOSurfaceGetWidth(self.raw.as_ptr()) }).unwrap_or(0)
    }

    pub fn height(&self) -> u32 {
        // SAFETY: the surface is live.
        u32::try_from(unsafe { IOSurfaceGetHeight(self.raw.as_ptr()) }).unwrap_or(0)
    }

    /// Bytes between rows, which may be more than the width takes.
    pub fn stride(&self) -> u32 {
        // SAFETY: the surface is live.
        u32::try_from(unsafe { IOSurfaceGetBytesPerRow(self.raw.as_ptr()) }).unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        // SAFETY: the surface is live.
        unsafe { IOSurfaceGetAllocSize(self.raw.as_ptr()) }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The surface for a host to put on a layer, with one more reference that's now the host's to release.
    pub fn retained_for_host(&self) -> u64 {
        // SAFETY: the surface is live, and the extra reference outlives this one until the host releases it.
        unsafe { CFRetain(self.raw.as_ptr().cast_const()) as usize as u64 }
    }

    /// Reaches the memory, under the surface's lock. A compromised worker that kept its own reference can still write here at any moment, so whatever's read is treated as untrusted bytes.
    pub fn with_pixels<R>(&self, step: impl FnOnce(&mut [u8]) -> R) -> io::Result<R> {
        let raw = self.raw.as_ptr();
        // SAFETY: locked before the address is read and until `step` ends. The length is the surface's own allocation, which can't change.
        unsafe {
            if IOSurfaceLock(raw, 0, std::ptr::null_mut()) != 0 {
                return Err(io::Error::other("the surface couldn't be locked"));
            }
            let base = IOSurfaceGetBaseAddress(raw).cast::<u8>();
            let result = if base.is_null() { Err(io::Error::other("the surface has no memory")) } else { Ok(step(std::slice::from_raw_parts_mut(base, IOSurfaceGetAllocSize(raw)))) };
            IOSurfaceUnlock(raw, 0, std::ptr::null_mut());
            result
        }
    }

    /// Lets the OS take this surface's memory if it needs to. It leaves the footprint at once, and a surface someone's holding on a layer must not be marked: only off-screen ones.
    pub fn set_discardable(&self) {
        let mut old = 0;
        // SAFETY: the surface is live, and `old` is valid for writes.
        unsafe { IOSurfaceSetPurgeable(self.raw.as_ptr(), PURGEABLE_VOLATILE, &raw mut old) };
    }

    /// Takes back a surface marked discardable, and says whether its pixels survived.
    pub fn reclaim(&self) -> Purge {
        let mut old = 0;
        // SAFETY: the surface is live, and `old` is valid for writes.
        let rc = unsafe { IOSurfaceSetPurgeable(self.raw.as_ptr(), PURGEABLE_NON_VOLATILE, &raw mut old) };
        if rc == 0 && old != PURGEABLE_EMPTY { Purge::Kept } else { Purge::Emptied }
    }

    /// Whether the OS has already taken its memory, without changing anything.
    pub fn was_emptied(&self) -> bool {
        let mut old = 0;
        // SAFETY: the surface is live, and `old` is valid for writes.
        let rc = unsafe { IOSurfaceSetPurgeable(self.raw.as_ptr(), PURGEABLE_KEEP_CURRENT, &raw mut old) };
        rc != 0 || old == PURGEABLE_EMPTY
    }
}

impl Clone for PixelSurface {
    fn clone(&self) -> Self {
        // SAFETY: the surface is live, and the new reference is released by the clone's drop.
        unsafe { CFRetain(self.raw.as_ptr().cast_const()) };
        Self { raw: self.raw }
    }
}

impl Drop for PixelSurface {
    fn drop(&mut self) {
        // SAFETY: this value owns one reference.
        unsafe { CFRelease(self.raw.as_ptr().cast_const()) };
    }
}

// Mach, for the one thing a socket can't carry. Not in the `libc` crate in this form.
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_MSG_TYPE_MOVE_SEND: u32 = 17;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_PORT_DESCRIPTOR: u32 = 0;
const MACH_SEND_MSG: c_int = 1;
const MACH_RCV_MSG: c_int = 2;
const MACH_SEND_TIMEOUT: c_int = 0x10;
const MACH_RCV_TIMEOUT: c_int = 0x100;
const EXCEPTION_DEFAULT: c_int = 1;
/// An exception nothing raises on this hardware: remote procedure call alerts. Its slot carries the port and nothing else, and it's cleared as soon as it's been read.
const LINK_SLOT_MASK: u32 = 1 << 9;
/// Tells a surface from a link's first message.
const MSG_LINK: i32 = 0x5245_4354;
const MSG_SURFACE: i32 = 0x5245_4355;

#[repr(C)]
#[derive(Clone, Copy)]
struct MsgHeader {
    bits: u32,
    size: u32,
    remote: MachPort,
    local: MachPort,
    voucher: MachPort,
    id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PortMessage {
    header: MsgHeader,
    descriptors: u32,
    name: MachPort,
    pad: u32,
    /// disposition in bits 16 to 23, type in bits 24 to 31.
    disposition_and_type: u32,
}

#[repr(C)]
struct PortReceived {
    message: PortMessage,
    trailer: [u8; 68],
}

unsafe extern "C" {
    fn mach_task_self() -> MachPort;
    fn mach_port_allocate(task: MachPort, right: u32, name: *mut MachPort) -> KernReturn;
    fn mach_port_insert_right(task: MachPort, name: MachPort, right: MachPort, disposition: u32) -> KernReturn;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    fn mach_port_mod_refs(task: MachPort, name: MachPort, right: u32, delta: c_int) -> KernReturn;
    fn mach_msg(message: *mut MsgHeader, option: c_int, send_size: u32, receive_limit: u32, receive_name: MachPort, timeout: u32, notify: MachPort) -> KernReturn;
    fn task_get_exception_ports(task: MachPort, mask: u32, masks: *mut u32, count: *mut u32, ports: *mut MachPort, behaviors: *mut c_int, flavors: *mut c_int) -> KernReturn;
    fn task_set_exception_ports(task: MachPort, mask: u32, port: MachPort, behavior: c_int, flavor: c_int) -> KernReturn;
    pub(crate) fn posix_spawnattr_setexceptionports_np(attr: *mut libc::posix_spawnattr_t, mask: u32, port: MachPort, behavior: c_int, flavor: c_int) -> c_int;
}

fn port_message(remote: MachPort, name: MachPort, disposition: u32, id: i32) -> PortMessage {
    PortMessage {
        header: MsgHeader { bits: MACH_MSG_TYPE_COPY_SEND | MACH_MSGH_BITS_COMPLEX, size: size_of::<PortMessage>() as u32, remote, local: 0, voucher: 0, id },
        descriptors: 1,
        name,
        pad: 0,
        disposition_and_type: (disposition << 16) | (MACH_MSG_PORT_DESCRIPTOR << 24),
    }
}

fn kern(rc: KernReturn, what: &str) -> io::Result<()> {
    if rc == 0 { Ok(()) } else { Err(io::Error::other(format!("{what}: {rc:#x}"))) }
}

fn millis(limit: Duration) -> u32 {
    u32::try_from(limit.as_millis()).unwrap_or(u32::MAX)
}

/// The host's half of a link while it's being made: what a child is started with.
#[derive(Debug)]
pub struct LinkInvite {
    /// Holds a receive right and a send right.
    port: MachPort,
}

impl LinkInvite {
    pub fn new() -> io::Result<Self> {
        let mut port = 0;
        // SAFETY: `port` is valid for writes, and on failure there's nothing to release.
        unsafe {
            let task = mach_task_self();
            kern(mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &raw mut port), "allocate")?;
            let invite = Self { port };
            kern(mach_port_insert_right(task, port, port, MACH_MSG_TYPE_MAKE_SEND), "insert right")?;
            Ok(invite)
        }
    }

    /// Sets the child's slot, for `posix_spawn`.
    ///
    /// # Safety
    /// `attr` must be an initialised spawn attribute.
    pub(crate) unsafe fn apply(&self, attr: *mut libc::posix_spawnattr_t) -> c_int {
        // SAFETY: as the caller promises. The port is live for as long as this value is.
        unsafe { posix_spawnattr_setexceptionports_np(attr, LINK_SLOT_MASK, self.port, EXCEPTION_DEFAULT, 0) }
    }

    /// Takes the link from what the child sent, if it has. A child that sent something else, or hasn't got that far, gives no link. It's read once and only for its shape, since the child may be hostile.
    pub fn accept(self, wait: Duration) -> Option<SurfaceSender> {
        let mut buffer = PortReceived { message: port_message(0, 0, 0, 0), trailer: [0; 68] };
        // SAFETY: the buffer's size is given as the receive limit, and the port is ours.
        let rc = unsafe { mach_msg(&raw mut buffer.message.header, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, size_of::<PortReceived>() as u32, self.port, millis(wait), 0) };
        if rc != 0 {
            return None;
        }
        let message = buffer.message;
        let complex = message.header.bits & MACH_MSGH_BITS_COMPLEX != 0;
        if !complex || message.descriptors != 1 || message.header.id != MSG_LINK || message.header.size != size_of::<PortMessage>() as u32 || message.name == 0 {
            // A message that isn't the link. Whatever rights it brought are left to this process's exit: a worker that does this is killed by whoever notices it has no link.
            return None;
        }
        Some(SurfaceSender { port: message.name })
    }
}

impl Drop for LinkInvite {
    fn drop(&mut self) {
        // SAFETY: this value owns the receive right and one send right on the name.
        unsafe {
            let task = mach_task_self();
            mach_port_deallocate(task, self.port);
            mach_port_mod_refs(task, self.port, MACH_PORT_RIGHT_RECEIVE, -1);
        }
    }
}

/// Sends surfaces to one worker.
#[derive(Debug)]
pub struct SurfaceSender {
    port: MachPort,
}

impl SurfaceSender {
    /// Hands the worker a surface. Never blocks: a worker that isn't reading, or has gone, is an error here.
    pub fn send(&self, surface: &PixelSurface) -> io::Result<()> {
        // SAFETY: the surface is live, and the right made here is moved into the message or released below.
        unsafe {
            let right = IOSurfaceCreateMachPort(surface.raw.as_ptr());
            if right == 0 {
                return Err(io::Error::other("the surface has no port"));
            }
            let mut message = port_message(self.port, right, MACH_MSG_TYPE_MOVE_SEND, MSG_SURFACE);
            let rc = mach_msg(&raw mut message.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, size_of::<PortMessage>() as u32, 0, 0, 0, 0);
            if rc != 0 {
                mach_port_deallocate(mach_task_self(), right);
                return Err(io::Error::other(format!("sending a surface: {rc:#x}")));
            }
        }
        Ok(())
    }
}

impl Drop for SurfaceSender {
    fn drop(&mut self) {
        // SAFETY: this value owns one send right.
        unsafe { mach_port_deallocate(mach_task_self(), self.port) };
    }
}

/// Receives surfaces, in the worker.
#[derive(Debug)]
pub struct SurfaceReceiver {
    port: MachPort,
}

impl SurfaceReceiver {
    /// Answers the invitation it was started with, if it was, and clears the slot. A process started without one has no link, and draws into shared memory instead.
    pub fn from_invite() -> Option<Self> {
        let (mut masks, mut count, mut ports, mut behaviors, mut flavors) = ([0u32; 32], 32u32, [0 as MachPort; 32], [0 as c_int; 32], [0 as c_int; 32]);
        // SAFETY: the arrays hold the 32 entries `count` says, and every right received here is this process's to use.
        unsafe {
            let task = mach_task_self();
            let rc = task_get_exception_ports(task, LINK_SLOT_MASK, masks.as_mut_ptr(), &raw mut count, ports.as_mut_ptr(), behaviors.as_mut_ptr(), flavors.as_mut_ptr());
            task_set_exception_ports(task, LINK_SLOT_MASK, 0, EXCEPTION_DEFAULT, 0);
            if rc != 0 || count == 0 || ports[0] == 0 || ports[0] == u32::MAX {
                return None;
            }
            let mut port = 0;
            if mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &raw mut port) != 0 {
                return None;
            }
            let mut message = port_message(ports[0], port, MACH_MSG_TYPE_MAKE_SEND, MSG_LINK);
            let sent = mach_msg(&raw mut message.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, size_of::<PortMessage>() as u32, 0, 0, 1000, 0);
            mach_port_deallocate(task, ports[0]);
            (sent == 0).then_some(Self { port })
        }
    }

    /// The next surface, or `None` if none comes in time. The host sends it before the frame that needs it, so this doesn't wait in practice.
    pub fn next(&self, wait: Duration) -> Option<PixelSurface> {
        let mut buffer = PortReceived { message: port_message(0, 0, 0, 0), trailer: [0; 68] };
        // SAFETY: the buffer's size is given as the receive limit, the port is ours, and the right received is released once it's been looked up.
        unsafe {
            if mach_msg(&raw mut buffer.message.header, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, size_of::<PortReceived>() as u32, self.port, millis(wait), 0) != 0 {
                return None;
            }
            let message = buffer.message;
            if message.header.bits & MACH_MSGH_BITS_COMPLEX == 0 || message.descriptors != 1 || message.header.id != MSG_SURFACE {
                return None;
            }
            let raw = IOSurfaceLookupFromMachPort(message.name);
            mach_port_deallocate(mach_task_self(), message.name);
            NonNull::new(raw).map(|raw| PixelSurface { raw })
        }
    }
}
