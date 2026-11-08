//! No sandbox here yet: the walls never go up, so a worker that requires them refuses to run. See `wall_mac.rs`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoWall {
    NotSandboxed,
    NothingToDrop,
    LaunchdReachable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachDrop {
    pub dropped: u32,
    pub kept: u32,
}

pub fn in_a_sandbox() -> bool {
    false
}

pub fn sandbox_in_force() -> bool {
    false
}

pub fn drop_mach_send_rights() -> MachDrop {
    MachDrop { dropped: 0, kept: 0 }
}

pub fn launchd_reachable() -> bool {
    false
}

pub fn put_up() -> Result<MachDrop, NoWall> {
    Err(NoWall::NotSandboxed)
}
