//! The renderer's walls, from the inside: whether the App Sandbox is really on, and letting go of every Mach send right that reaches another process.
//!
//! The sandbox comes from the helper's signature (`com.apple.security.app-sandbox`), so a build signed wrong runs without it and nothing says so. [`put_up`] asks the kernel whether
//! this process is sandboxed and checks the App Sandbox has given it a container, and the worker refuses to read a byte of a document when it hasn't. The App Sandbox alone still lets a process launch any app through LaunchServices, and the app
//! it launches runs outside the sandbox (`docs/research/2026-11-07-sandbox-spike.md`, "The hole"). No entitlement takes that away, so the second wall drops every send right that isn't
//! the kernel's. A Mach service can only be reached through a send right, and a process can't make one, so with none left nothing can be asked for: not launchd, LaunchServices, the
//! pasteboard or the window server. Public calls from `<mach/*.h>`. It isn't a documented security boundary: it's behind the sandbox, not instead of it.

use std::ffi::{c_char, c_int};

unsafe extern "C" {
    static mach_task_self_: u32;
    static bootstrap_port: u32;
    fn mach_port_names(task: u32, names: *mut *mut u32, names_count: *mut u32, types: *mut *mut u32, types_count: *mut u32) -> c_int;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: c_int) -> c_int;
    fn mach_port_allocate_name(task: u32, right: u32, name: u32) -> c_int;
    fn mach_port_insert_right(task: u32, name: u32, port: u32, disposition: u32) -> c_int;
    fn mach_port_get_refs(task: u32, name: u32, right: u32, refs: *mut u32) -> c_int;
    fn mach_port_kernel_object(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
    fn mach_ports_register(task: u32, ports: *mut u32, count: u32) -> c_int;
    fn task_set_special_port(task: u32, which: c_int, port: u32) -> c_int;
    fn task_get_special_port(task: u32, which: c_int, port: *mut u32) -> c_int;
    fn task_set_exception_ports(task: u32, mask: u32, port: u32, behavior: c_int, flavor: c_int) -> c_int;
    fn vm_deallocate(task: u32, address: usize, size: usize) -> c_int;
    fn mach_port_kobject(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
    fn bootstrap_look_up(bp: u32, name: *const c_char, port: *mut u32) -> c_int;
    /// libsandbox, in libSystem. In no public header, and the documented way to ask: Chromium and WebKit ask the same question this way. 1 is sandboxed, 0 isn't, -1 couldn't tell.
    fn sandbox_check(pid: c_int, operation: *const c_char, filter: c_int, ...) -> c_int;
}

const MACH_PORT_TYPE_SEND: u32 = 1 << 16;
const MACH_PORT_RIGHT_SEND: u32 = 0;
const MACH_PORT_RIGHT_RECEIVE: u32 = 1;
const MACH_PORT_TYPE_RECEIVE: u32 = 1 << 17;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const TASK_BOOTSTRAP_PORT: c_int = 4;
const TASK_ACCESS_PORT: c_int = 9;
const TASK_DEBUG_CONTROL_PORT: c_int = 10;
const EXC_MASK_ALL: u32 = 0x3ffe;
const EXCEPTION_DEFAULT: c_int = 1;

/// Why the walls aren't up. The worker exits on any of them before a document reaches it, each with its own code, so the host can say which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoWall {
    /// The kernel says this process isn't sandboxed, or it is but the App Sandbox gave it no container: the helper was signed without the sandbox, or this is some other sandbox.
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
    /// Of those, the names left behind as dead names, so a library that held one can still let go of it.
    pub dead_names: u32,
    /// Kept: the kernel's own, such as this task, its threads and the host, which reach no process, send rights to this process's own ports, and any the kernel wouldn't describe, as dropping a right a library still uses
    /// could crash it later. What decides the wall is that launchd doesn't answer afterwards, which [`put_up`] checks.
    pub kept: u32,
}

/// Whether the kernel has this process in any sandbox at all. True inside an agent's seatbelt too, so on its own it doesn't say the App Sandbox is on. Asks; logs nothing.
pub fn in_a_sandbox() -> bool {
    // SAFETY: a null operation with no filter asks only whether there's a sandbox, and passes nothing to read.
    unsafe { sandbox_check(libc::getpid(), std::ptr::null(), 0) == 1 }
}

/// Whether the App Sandbox is in force on this process: the kernel says it's sandboxed, and the App Sandbox gave it a container, which it says in `APP_SANDBOX_CONTAINER_ID`. The host
/// starts a worker with an empty environment, so only the App Sandbox can have put that there, and an inherited seatbelt alone, which sets nothing, reads as no. Both are positive
/// answers, so nothing here depends on where the user's home folder is, and nothing is denied or logged.
pub fn sandbox_in_force() -> bool {
    in_a_sandbox() && std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some_and(|id| !id.is_empty())
}

/// What the kernel says a right names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    /// A kernel object: this task, a thread, the host, a semaphore, a driver's connection.
    Kernel,
    /// A port some process receives on.
    Process,
    /// The kernel wouldn't say.
    Unknown,
}

/// Asks the kernel what a send right names: `mach_port_kobject`, then the older `mach_port_kernel_object`, both in `<mach/mach_port.h>`. A port no kernel object stands behind reads as
/// type 0 (none) or all ones (unknown), and a kernel object as its small type number. If both calls fail, it's `Unknown`, and the right is kept.
fn named(task: u32, name: u32) -> Named {
    let (mut object_type, mut address) = (0u32, 0u64);
    // SAFETY: both out-pointers are live, and these only read about a name this process holds. The older call writes a 32-bit address into the first half of `address`.
    let rc = unsafe {
        let mut rc = mach_port_kobject(task, name, &raw mut object_type, &raw mut address);
        if rc != 0 {
            rc = mach_port_kernel_object(task, name, &raw mut object_type, &raw mut address);
        }
        rc
    };
    match (rc, object_type) {
        (0, 0 | u32::MAX) => Named::Process,
        (0, _) => Named::Kernel,
        _ => Named::Unknown,
    }
}

/// Lets go of every send right to a process, and clears every slot a later call could fetch one back from: the task's special ports, its registered ports and its exception ports.
/// Launchd's right goes whatever the kernel says of it. With the exception ports gone this process makes no crash report: the host reads the exit status, and a report would hold
/// hostile bytes anyway. Receive rights stay, so a link made before this, such as the one surfaces come over, keeps working.
pub fn drop_mach_send_rights() -> MachDrop {
    // SAFETY: reading variables the runtime set before `main`.
    let (task, launchd) = unsafe { (mach_task_self_, bootstrap_port) };
    let mut special = 0u32;
    // SAFETY: `special` is live, and this reads only this task's own slot. It names the same port as `bootstrap_port` unless something replaced it.
    unsafe { task_get_special_port(task, TASK_BOOTSTRAP_PORT, &raw mut special) };
    let mut names: *mut u32 = std::ptr::null_mut();
    let mut types: *mut u32 = std::ptr::null_mut();
    let (mut name_count, mut type_count) = (0u32, 0u32);
    // SAFETY: all four out-pointers are live. The arrays the kernel returns are read, then given back below.
    if unsafe { mach_port_names(task, &raw mut names, &raw mut name_count, &raw mut types, &raw mut type_count) } != 0 {
        return MachDrop {
            dropped: 0,
            dead_names: 0,
            kept: 0,
        };
    }
    // SAFETY: the kernel returned that many of each.
    let (name_list, type_list) = unsafe {
        (
            std::slice::from_raw_parts(names, name_count as usize),
            std::slice::from_raw_parts(types, type_count as usize),
        )
    };
    // SAFETY: only this task's own ports, with null as the new value. Launchd's right, and the two others launchd leaves here.
    unsafe {
        for which in [TASK_BOOTSTRAP_PORT, TASK_ACCESS_PORT, TASK_DEBUG_CONTROL_PORT] {
            task_set_special_port(task, which, 0);
        }
        mach_ports_register(task, std::ptr::null_mut(), 0);
        task_set_exception_ports(task, EXC_MASK_ALL, 0, EXCEPTION_DEFAULT, 0);
    }
    let mut outcome = MachDrop {
        dropped: 0,
        dead_names: 0,
        kept: 0,
    };
    for (name, kind) in name_list.iter().zip(type_list) {
        if kind & MACH_PORT_TYPE_SEND == 0 {
            continue;
        }
        // A send right to a port this process receives on reaches only this process. Libraries hold those for their own queues, and taking one away is a guard exception later.
        if kind & MACH_PORT_TYPE_RECEIVE != 0 {
            outcome.kept += 1;
            continue;
        }
        let is_launchd = *name != 0 && (*name == launchd || *name == special);
        if !is_launchd && named(task, *name) != Named::Process {
            outcome.kept += 1;
            continue;
        }
        let mut refs = 0u32;
        // SAFETY: `refs` is live, and these touch only one of this process's own names. All its send references go, so the right does too.
        let rc = unsafe {
            mach_port_get_refs(task, *name, MACH_PORT_RIGHT_SEND, &raw mut refs);
            mach_port_mod_refs(task, *name, MACH_PORT_RIGHT_SEND, -(refs as c_int))
        };
        if rc == 0 {
            outcome.dropped += 1;
            if leave_a_dead_name(task, *name, refs) {
                outcome.dead_names += 1;
            }
        } else {
            outcome.kept += 1;
        }
    }
    // SAFETY: the two arrays came from `mach_port_names` with these counts, and nothing reads them after this.
    unsafe {
        vm_deallocate(task, names as usize, name_count as usize * size_of::<u32>());
        vm_deallocate(task, types as usize, type_count as usize * size_of::<u32>());
    }
    outcome
}

/// Puts a dead name back where a send right was, with as many references, so the libraries that held it can still let go of it. Without this, their later
/// `mach_port_deallocate` names nothing, and the kernel raises a Mach port guard exception (`kGUARD_EXC_INVALID_NAME`): logged on this Mac, about one a worker, 15,816 in a morning
/// of tests, and fatal on a CI runner, which killed a process partway through its work. A dead name reaches nothing: a message to it fails, and letting go of it is fine. Made by
/// taking the name for a port of this process's own, adding the send references, and destroying the port, which leaves them as a dead name. If the name has been taken in the
/// meantime, it's left as it is: the right is gone either way. Says whether a dead name is there now.
fn leave_a_dead_name(task: u32, name: u32, refs: u32) -> bool {
    // SAFETY: only this process's own space, under a name it just freed. The receive right made here is destroyed before this returns.
    unsafe {
        if mach_port_allocate_name(task, MACH_PORT_RIGHT_RECEIVE, name) != 0 {
            return false;
        }
        let sent = mach_port_insert_right(task, name, name, MACH_MSG_TYPE_MAKE_SEND) == 0;
        if sent && refs > 1 {
            mach_port_mod_refs(task, name, MACH_PORT_RIGHT_SEND, refs as c_int - 1);
        }
        mach_port_mod_refs(task, name, MACH_PORT_RIGHT_RECEIVE, -1);
        sent
    }
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

/// Both walls, in order, checked: the App Sandbox is on, the rights are gone, and launchd doesn't answer. For a worker after its library has loaded and before any hostile byte arrives,
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
        assert!(!in_a_sandbox());
        assert!(!sandbox_in_force());
        assert!(
            launchd_reachable(),
            "a plain process reaches launchd, so the check after the drop can see a difference"
        );
    }
}
