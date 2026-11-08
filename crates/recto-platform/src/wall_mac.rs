//! The renderer's walls, from the inside: whether the App Sandbox is really on, and letting go of every Mach send right that reaches another process.
//!
//! The sandbox comes from the helper's signature (`com.apple.security.app-sandbox`), so a build signed wrong runs without it and nothing says so. [`put_up`] tests the wall itself rather
//! than trusting a flag, and the worker refuses to read a byte of a document when it fails. The App Sandbox alone still lets a process launch any app through LaunchServices, and the app
//! it launches runs outside the sandbox (`docs/research/2026-11-07-sandbox-spike.md`, "The hole"). No entitlement takes that away, so the second wall drops every send right that isn't
//! the kernel's. A Mach service can only be reached through a send right, and a process can't make one, so with none left nothing can be asked for: not launchd, LaunchServices, the
//! pasteboard or the window server. Public calls from `<mach/*.h>`. It isn't a documented security boundary: it's behind the sandbox, not instead of it.

use std::ffi::{c_char, c_int};

unsafe extern "C" {
    static mach_task_self_: u32;
    static bootstrap_port: u32;
    fn mach_port_names(task: u32, names: *mut *mut u32, names_count: *mut u32, types: *mut *mut u32, types_count: *mut u32) -> c_int;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: c_int) -> c_int;
    fn mach_port_get_refs(task: u32, name: u32, right: u32, refs: *mut u32) -> c_int;
    fn mach_port_kernel_object(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
    fn mach_ports_register(task: u32, ports: *mut u32, count: u32) -> c_int;
    fn task_set_special_port(task: u32, which: c_int, port: u32) -> c_int;
    fn task_get_special_port(task: u32, which: c_int, port: *mut u32) -> c_int;
    fn task_set_exception_ports(task: u32, mask: u32, port: u32, behavior: c_int, flavor: c_int) -> c_int;
    fn vm_deallocate(task: u32, address: usize, size: usize) -> c_int;
    fn bootstrap_look_up(bp: u32, name: *const c_char, port: *mut u32) -> c_int;
}

const MACH_PORT_TYPE_SEND: u32 = 1 << 16;
const MACH_PORT_RIGHT_SEND: u32 = 0;
const TASK_BOOTSTRAP_PORT: c_int = 4;
const TASK_ACCESS_PORT: c_int = 9;
const TASK_DEBUG_CONTROL_PORT: c_int = 10;
const EXC_MASK_ALL: u32 = 0x3ffe;
const EXCEPTION_DEFAULT: c_int = 1;

/// Why the walls aren't up. The worker exits on any of them before a document reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoWall {
    /// A folder every plain process can list was listable, or refused for a reason that isn't the sandbox's. So the App Sandbox isn't on: the helper was signed without it.
    NotSandboxed,
    /// No right was there to drop, so this process isn't as this code understands a process to be, and the Mach wall can't be vouched for.
    NothingToDrop,
    /// After the drop, launchd still answered a lookup.
    LaunchdReachable,
}

/// What the Mach drop did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachDrop {
    /// Send rights let go of.
    pub dropped: u32,
    /// Kept because they're the kernel's: this task, its threads, the host, a semaphore. Threads and memory calls need them, and they reach no process.
    pub kept: u32,
}

/// Whether the App Sandbox is in force on this process. The test is the wall itself: `/Users` is listable by any plain process and refused inside the sandbox, with `EPERM`, which is how
/// the sandbox refuses. Anything else, a listing, `ENOENT` or `EACCES`, counts as no sandbox, so a strange Mac fails closed. One denied read, logged by the system as a sandbox violation.
pub fn sandbox_in_force() -> bool {
    matches!(std::fs::read_dir("/Users"), Err(error) if error.raw_os_error() == Some(libc::EPERM))
}

/// Lets go of every send right that isn't the kernel's, and clears every slot a later call could fetch one back from: the task's special ports, its registered ports and its exception
/// ports. With the exception ports gone this process makes no crash report: the host reads the exit status, and a report would hold hostile bytes anyway. Receive rights stay, so a
/// link made before this, such as the one surfaces come over, keeps working.
pub fn drop_mach_send_rights() -> MachDrop {
    // SAFETY: reading a variable the runtime set before `main`.
    let task = unsafe { mach_task_self_ };
    let mut names: *mut u32 = std::ptr::null_mut();
    let mut types: *mut u32 = std::ptr::null_mut();
    let (mut name_count, mut type_count) = (0u32, 0u32);
    // SAFETY: all four out-pointers are live. The arrays the kernel returns are read, then given back below.
    if unsafe { mach_port_names(task, &raw mut names, &raw mut name_count, &raw mut types, &raw mut type_count) } != 0 {
        return MachDrop { dropped: 0, kept: 0 };
    }
    // SAFETY: the kernel returned that many of each.
    let (name_list, type_list) = unsafe { (std::slice::from_raw_parts(names, name_count as usize), std::slice::from_raw_parts(types, type_count as usize)) };
    // SAFETY: only this task's own ports, with null as the new value. Launchd's right, and the two others launchd leaves here.
    unsafe {
        for which in [TASK_BOOTSTRAP_PORT, TASK_ACCESS_PORT, TASK_DEBUG_CONTROL_PORT] {
            task_set_special_port(task, which, 0);
        }
        mach_ports_register(task, std::ptr::null_mut(), 0);
        task_set_exception_ports(task, EXC_MASK_ALL, 0, EXCEPTION_DEFAULT, 0);
    }
    let mut outcome = MachDrop { dropped: 0, kept: 0 };
    for (name, kind) in name_list.iter().zip(type_list) {
        if kind & MACH_PORT_TYPE_SEND == 0 {
            continue;
        }
        let (mut object_type, mut address) = (0u32, 0u64);
        // SAFETY: both out-pointers are live, and this only reads about a name this process holds.
        let described = unsafe { mach_port_kernel_object(task, *name, &raw mut object_type, &raw mut address) };
        // A port some process receives on reads as "unknown", `u32::MAX`. A kernel object reads as its small type number.
        if described == 0 && object_type != 0 && object_type != u32::MAX {
            outcome.kept += 1;
            continue;
        }
        let mut refs = 0u32;
        // SAFETY: `refs` is live, and these touch only one of this process's own names. All its send references go, so the right does too.
        let rc = unsafe {
            mach_port_get_refs(task, *name, MACH_PORT_RIGHT_SEND, &raw mut refs);
            mach_port_mod_refs(task, *name, MACH_PORT_RIGHT_SEND, -(refs as c_int))
        };
        if rc == 0 { outcome.dropped += 1 } else { outcome.kept += 1 }
    }
    // SAFETY: the two arrays came from `mach_port_names` with these counts, and nothing reads them after this.
    unsafe {
        vm_deallocate(task, names as usize, name_count as usize * size_of::<u32>());
        vm_deallocate(task, types as usize, type_count as usize * size_of::<u32>());
    }
    outcome
}

/// Whether launchd can still be reached: its slot holds a right, or a lookup through the right this process started with gets an answer. Asks for a port and sends the service nothing.
pub fn launchd_reachable() -> bool {
    let mut port = 0u32;
    // SAFETY: `port` is live, and both calls only read this task's own rights.
    unsafe {
        if task_get_special_port(mach_task_self_, TASK_BOOTSTRAP_PORT, &raw mut port) == 0 && port != 0 {
            return true;
        }
        let mut service = 0u32;
        bootstrap_look_up(bootstrap_port, c"com.apple.coreservices.launchservicesd".as_ptr(), &raw mut service) == 0
    }
}

/// Both walls, in order, checked: the sandbox is on, the rights are gone, and launchd doesn't answer. For a worker after its library has loaded and before any hostile byte arrives,
/// since loading asks `cfprefsd` and `lsd.mapdb` for things and nothing after does.
pub fn put_up() -> Result<MachDrop, NoWall> {
    if !sandbox_in_force() {
        return Err(NoWall::NotSandboxed);
    }
    let dropped = drop_mach_send_rights();
    if dropped.dropped == 0 {
        return Err(NoWall::NothingToDrop);
    }
    if launchd_reachable() {
        return Err(NoWall::LaunchdReachable);
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the wall test reads a plain process as sandboxed, so a helper signed without the sandbox would draw hostile files. This test process isn't sandboxed.
    #[test]
    fn a_plain_process_is_not_read_as_sandboxed() {
        assert!(!sandbox_in_force());
        assert!(launchd_reachable(), "a plain process reaches launchd, so the check after the drop can see a difference");
    }
}
