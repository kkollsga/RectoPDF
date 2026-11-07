//! The little of libxpc the spike uses, and a captureless block. No new crate: a block with no captures is a fixed struct.
#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type XpcObject = *mut c_void;

#[repr(C)]
pub struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

#[repr(C)]
pub struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*const Block, XpcObject),
    descriptor: *const BlockDescriptor,
}

unsafe extern "C" {
    static _NSConcreteGlobalBlock: [*const c_void; 32];
    pub static _xpc_type_dictionary: c_void;
    pub static _xpc_type_error: c_void;
    pub static _xpc_error_connection_interrupted: c_void;
    pub static _xpc_error_connection_invalid: c_void;
    pub fn xpc_main(handler: extern "C" fn(XpcObject)) -> !;
    pub fn xpc_connection_create(name: *const c_char, queue: *mut c_void) -> XpcObject;
    pub fn xpc_connection_set_event_handler(connection: XpcObject, handler: *const Block);
    pub fn xpc_connection_resume(connection: XpcObject);
    pub fn xpc_connection_cancel(connection: XpcObject);
    pub fn xpc_connection_get_pid(connection: XpcObject) -> libc::pid_t;
    pub fn xpc_connection_send_message(connection: XpcObject, message: XpcObject);
    pub fn xpc_connection_send_message_with_reply_sync(connection: XpcObject, message: XpcObject) -> XpcObject;
    pub fn xpc_get_type(object: XpcObject) -> *const c_void;
    pub fn xpc_dictionary_create(keys: *const *const c_char, values: *const XpcObject, count: usize) -> XpcObject;
    pub fn xpc_dictionary_create_reply(original: XpcObject) -> XpcObject;
    pub fn xpc_dictionary_get_remote_connection(dictionary: XpcObject) -> XpcObject;
    pub fn xpc_dictionary_set_string(dictionary: XpcObject, key: *const c_char, value: *const c_char);
    pub fn xpc_dictionary_get_string(dictionary: XpcObject, key: *const c_char) -> *const c_char;
    pub fn xpc_dictionary_set_int64(dictionary: XpcObject, key: *const c_char, value: i64);
    pub fn xpc_dictionary_get_int64(dictionary: XpcObject, key: *const c_char) -> i64;
    pub fn xpc_dictionary_set_fd(dictionary: XpcObject, key: *const c_char, fd: c_int);
    pub fn xpc_dictionary_dup_fd(dictionary: XpcObject, key: *const c_char) -> c_int;
    pub fn xpc_release(object: XpcObject);
    pub fn xpc_retain(object: XpcObject) -> XpcObject;
    pub fn xpc_transaction_begin();
}

/// A block that calls `invoke`, for ever. Leaked, since a connection's handler lives as long as the connection and these are few.
pub fn global_block(invoke: extern "C" fn(*const Block, XpcObject)) -> *const Block {
    let descriptor = Box::leak(Box::new(BlockDescriptor { reserved: 0, size: size_of::<Block>() }));
    // BLOCK_IS_GLOBAL.
    let flags = 1 << 28;
    // SAFETY: only the address of the class object is taken, which is what a global block's `isa` is.
    let isa = (&raw const _NSConcreteGlobalBlock).cast();
    Box::leak(Box::new(Block { isa, flags, reserved: 0, invoke, descriptor }))
}

/// Makes a `Channel` from a raw socket, or the reverse: its one field is the descriptor, and the production type offers no way to say so. Spike only: production would add the
/// accessor if XPC were chosen.
pub fn channel_from_fd(fd: std::os::fd::OwnedFd) -> recto_platform::isolate::Channel {
    // SAFETY: `Channel` is a struct of one `OwnedFd`, so the two have the same layout and the same rules. The size check is the compiler's.
    unsafe { std::mem::transmute(fd) }
}

/// See [`channel_from_fd`].
pub fn fd_from_channel(channel: recto_platform::isolate::Channel) -> std::os::fd::OwnedFd {
    // SAFETY: as in `channel_from_fd`.
    unsafe { std::mem::transmute(channel) }
}
