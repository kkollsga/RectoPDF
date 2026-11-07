//! The real worker as an XPC service. A `start` message brings the far end of a socket pair and the worker's usual arguments, and from then on the socket carries the same frames as
//! the plain worker: XPC only launches the process and hands over the descriptor. A `ping` answers with the process id, and a `probe` runs the walls from `probe_core.rs` in here.

include!(concat!(env!("OUT_DIR"), "/worker_body.rs"));

#[path = "../mach_drop.rs"]
mod mach_drop;
#[path = "../probe_core.rs"]
mod probe_core;
#[path = "../xpc_ffi.rs"]
mod xpc_ffi;

use std::ffi::{CStr, c_char};
use std::os::fd::FromRawFd;
use std::sync::OnceLock;
use xpc_ffi::*;

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
}

static WORKER_ARGS: OnceLock<Vec<std::ffi::OsString>> = OnceLock::new();
static WORKER_FD: std::sync::Mutex<Option<std::os::fd::OwnedFd>> = std::sync::Mutex::new(None);

fn spike_args() -> Vec<std::ffi::OsString> {
    WORKER_ARGS.get().cloned().unwrap_or_default()
}

fn spike_channel() -> std::io::Result<Channel> {
    let fd = WORKER_FD.lock().unwrap().take().ok_or(std::io::ErrorKind::NotFound)?;
    Ok(channel_from_fd(fd))
}

fn spike_after_load() {}

fn text(message: XpcObject, key: &CStr) -> Option<String> {
    // SAFETY: `message` is a live dictionary and `key` is NUL-terminated. The result, if any, lives as long as the dictionary, and is copied here.
    unsafe {
        let value: *const c_char = xpc_dictionary_get_string(message, key.as_ptr());
        (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
    }
}

extern "C" fn on_message(_block: *const Block, message: XpcObject) {
    // SAFETY: every call is given the live message and NUL-terminated keys, and the reply is made from the message it answers.
    unsafe {
        if xpc_get_type(message) != (&raw const _xpc_type_dictionary).cast() {
            return;
        }
        let reply = xpc_dictionary_create_reply(message);
        // A message that wants no answer has no reply to make. These are experiments on why a page takes several times as long here: `hold` keeps the message alive, which is how XPC
        // keeps a service boosted while a request is open, and `unclamp` asks the kernel directly to take this process out of the background role.
        if reply.is_null() {
            if text(message, c"hold").is_some() {
                xpc_retain(message);
            }
            if text(message, c"unclamp").is_some() {
                libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, 0);
            }
            return;
        }
        xpc_dictionary_set_int64(reply, c"pid".as_ptr(), i64::from(libc::getpid()));
        if let Some(args) = text(message, c"probe") {
            probe_core::run(args.split('\u{1f}').map(str::to_string).collect());
            let report = std::ffi::CString::new(probe_core::take_report()).unwrap_or_default();
            xpc_dictionary_set_string(reply, c"report".as_ptr(), report.as_ptr());
        }
        if let Some(args) = text(message, c"start") {
            // One process per application, so a second document has nowhere to go.
            let refused = WORKER_ARGS.set(args.split('\u{1f}').map(Into::into).collect()).is_err();
            xpc_dictionary_set_int64(reply, c"refused".as_ptr(), i64::from(refused));
            let fd = xpc_dictionary_dup_fd(message, c"fd".as_ptr());
            if !refused && fd >= 0 {
                *WORKER_FD.lock().unwrap() = Some(std::os::fd::OwnedFd::from_raw_fd(fd));
                // The system may end an idle service that holds no transaction. A worker between jobs looks idle to XPC, as the socket is outside it.
                xpc_transaction_begin();
                // A service launched by XPC is clamped low unless a message boosts it, and the socket carries no boost: without this a page takes 7 times as long.
                let _ = std::thread::Builder::new().stack_size(8 << 20).spawn(|| {
                    pthread_set_qos_class_self_np(0x21, 0);
                    worker_main();
                });
            }
        }
        xpc_connection_send_message(xpc_dictionary_get_remote_connection(message), reply);
        xpc_release(reply);
    }
}

extern "C" fn on_connection(connection: XpcObject) {
    // SAFETY: the connection is live, and the block lives for ever.
    unsafe {
        xpc_connection_set_event_handler(connection, global_block(on_message));
        xpc_connection_resume(connection);
    }
}

fn main() {
    // SAFETY: the handler is a plain function that lives for ever. Doesn't return.
    unsafe { xpc_main(on_connection) }
}
