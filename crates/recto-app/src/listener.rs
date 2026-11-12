//! **What the app core tells the host.** The host implements [`Listener`] (`recto-ffi` over its UniFFI listener, a test over [`crate::testkit::FakeListener`]); the core's modules
//! take it as a generic parameter, so a call is one call into the host and never a trait object over another trait object.
//!
//! It grows with the modules that move here (step R1.2): each call joins it when the first module that makes it does, with the same promise as its UniFFI twin.

/// What the host hears from the core. Every call is news with no payload: the host pulls what changed, on its own thread, when it's ready.
///
/// **What the core promises the host, for every call:**
/// - **Thread:** any of the core's threads. Most come from the session thread; some from a worker (a print, a copy) and some from the UI thread itself, inside a call the host
///   made (opening the palette). The host never assumes which, and hops to its UI thread before it touches anything it draws.
/// - **Locks:** the core makes the call with none of its own locks held, so a host that pulls at once can't deadlock on the core.
/// - **Ordering:** a call comes after the change it announces can be seen by a pull. Calls may be coalesced: many calls promise only that a pull after the last one sees
///   everything they announced. Nothing is promised about the order of one kind of call against another.
///
/// **What the host promises the core:** each call returns at once. It doesn't block, wait for its UI thread or call back into the core before returning; it may be called
/// many times a second, so it does no more than note that a pull is owed.
pub trait Listener: Send + Sync {
    /// The window's view models changed: the sidebar, the reader, Find, a tool. The host pulls them.
    fn model_changed(&self);
}
