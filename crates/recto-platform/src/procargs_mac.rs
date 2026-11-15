//! **What another process of this user's was started with:** its arguments and environment as the kernel keeps them (`KERN_PROCARGS2`), which `ps -E` shows to anyone running as the same user. For a test that a secret was never put there: the Find key goes to the indexer on its stdin.

/// The process's arguments and environment, as the kernel's bytes. `None` if it's gone or can't be looked at.
pub fn arguments_and_environment(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, i32::try_from(pid).ok()?];
    let mut size: libc::size_t = 0;
    // SAFETY: a query of the size, with no buffer.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    let mut bytes = vec![0u8; size];
    // SAFETY: a buffer of the size the kernel said, and that size given with it.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, bytes.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    bytes.truncate(size);
    Some(bytes)
}
