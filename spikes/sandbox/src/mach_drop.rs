//! Lets go of every right to send to anything but the kernel. A Mach service can only be reached by a send right, and a process can't conjure one, so with none left (launchd's
//! included) nothing can be asked for: not LaunchServices, not the pasteboard, not the window server. The App Sandbox leaves those three reachable, and has no entitlement to take
//! them away. Public calls from `<mach/*.h>`. It isn't a documented security boundary, so it's a second wall behind the sandbox, not instead of it.

use std::ffi::c_int;

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_port_names(task: u32, names: *mut *mut u32, names_count: *mut u32, types: *mut *mut u32, types_count: *mut u32) -> c_int;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: c_int) -> c_int;
    fn mach_port_get_refs(task: u32, name: u32, right: u32, refs: *mut u32) -> c_int;
    fn mach_port_kernel_object(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
    fn mach_ports_register(task: u32, ports: *mut u32, count: u32) -> c_int;
    fn task_set_special_port(task: u32, which: c_int, port: u32) -> c_int;
    fn task_set_exception_ports(task: u32, mask: u32, port: u32, behavior: c_int, flavor: c_int) -> c_int;
}

const MACH_PORT_TYPE_SEND: u32 = 1 << 16;
const MACH_PORT_RIGHT_SEND: u32 = 0;
const TASK_BOOTSTRAP_PORT: c_int = 4;
const TASK_ACCESS_PORT: c_int = 9;
const TASK_DEBUG_CONTROL_PORT: c_int = 10;
const EXC_MASK_ALL: u32 = 0x3ffe;
const EXCEPTION_DEFAULT: c_int = 1;

/// Returns how many rights went and how many stayed because they're the kernel's: this task, a thread, the host, a semaphore. Threads and memory calls need those, and they reach no
/// process. The caller decides whether zero dropped is an error.
pub fn drop_send_rights() -> (u32, u32) {
    // SAFETY: reading a variable the runtime set before `main`.
    let task = unsafe { mach_task_self_ };
    let mut names: *mut u32 = std::ptr::null_mut();
    let mut types: *mut u32 = std::ptr::null_mut();
    let (mut name_count, mut type_count) = (0u32, 0u32);
    // SAFETY: all four out-pointers are live. The arrays the kernel returns are only read, and stay allocated, as this runs once in a process's life.
    if unsafe { mach_port_names(task, &raw mut names, &raw mut name_count, &raw mut types, &raw mut type_count) } != 0 {
        return (0, 0);
    }
    // SAFETY: the kernel returned that many of each.
    let (names, types) = unsafe { (std::slice::from_raw_parts(names, name_count as usize), std::slice::from_raw_parts(types, type_count as usize)) };
    // SAFETY: only this task's own ports, with null as the new value. Clears every place a later call could fetch a right to some process from: launchd's, and the two others
    // launchd leaves here. The exception port goes too, so this process makes no crash report: the host reads the exit status, and a report would hold hostile bytes anyway.
    unsafe {
        for which in [TASK_BOOTSTRAP_PORT, TASK_ACCESS_PORT, TASK_DEBUG_CONTROL_PORT] {
            task_set_special_port(task, which, 0);
        }
        mach_ports_register(task, std::ptr::null_mut(), 0);
        task_set_exception_ports(task, EXC_MASK_ALL, 0, EXCEPTION_DEFAULT, 0);
    }
    let (mut dropped, mut kept) = (0, 0);
    for (name, kind) in names.iter().zip(types) {
        if kind & MACH_PORT_TYPE_SEND == 0 {
            continue;
        }
        let (mut object_type, mut address) = (0u32, 0u64);
        // SAFETY: both out-pointers are live, and this only reads about a name this process holds.
        let described = unsafe { mach_port_kernel_object(task, *name, &raw mut object_type, &raw mut address) };
        // A port some process receives on reads as "unknown", -1. A kernel object reads as its small type number.
        if described == 0 && object_type != 0 && object_type != u32::MAX {
            kept += 1;
            continue;
        }
        let mut refs = 0u32;
        // SAFETY: `refs` is live, and these touch only one of this process's own names. All its send references go, so the right does too.
        let rc = unsafe {
            mach_port_get_refs(task, *name, MACH_PORT_RIGHT_SEND, &raw mut refs);
            mach_port_mod_refs(task, *name, MACH_PORT_RIGHT_SEND, -(refs as c_int))
        };
        if rc == 0 { dropped += 1 } else { kept += 1 }
    }
    (dropped, kept)
}
