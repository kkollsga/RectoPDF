//! Generated contents in the sidebar: what a tap on the note does and how a tab comes to show them are `crate::generated_contents`'s; this is how their answers reach the
//! session thread, through its queue.

use super::{CoreCall, Inner};
use crate::generated_contents as app;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The queue, as the one way an answer gets back. Made per call, an `Arc` clone: a look's thread takes its own, and is counted as under way while it holds
/// it, so until after it has queued what it found ([`Counted`]).
fn push(inner: &Inner) -> impl app::Push {
    let core = Arc::clone(&inner.core);
    let counted = Counted::new(inner.tools.contents_looks());
    move |id, view, version| {
        let _counted = &counted;
        core.push(CoreCall::GeneratedView(id, view, version));
    }
}

/// **A way back held, counted:** one more for each copy, one fewer as each goes. A look's thread holds its own copy until it ends, after it has queued its
/// answer, so the app's settled sees it under way till then ([`crate::settled`]); the copy the session thread makes for a call goes with the call.
struct Counted(Arc<AtomicUsize>);

impl Counted {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(count))
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        Self::new(&self.0)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn look_again(inner: &Inner) {
    app::look_again(&inner.host.session, &push(inner));
}

pub fn look_if_chosen(inner: &Inner) {
    app::look_if_chosen(&inner.host.session, &push(inner));
}

#[cfg(test)]
mod tests {
    use crate::testkit::headless::HeadlessApp;

    /// Failure (review of `build/settled-gaps`, 2026-11-14): a look at where a tab's generated contents stand runs on a thread of its own and queues what it
    /// found, but the app's settled didn't count it, so `qa wait` could answer before the sidebar's contents knew. A look's thread holds its own way back
    /// until it ends, after it has queued: while it holds it, the look is under way, and once it's let go nothing is.
    #[test]
    fn a_contents_look_is_under_way_while_it_holds_its_way_back() {
        let app = HeadlessApp::start();
        let for_the_call = super::push(app.core());
        let for_the_look = for_the_call.clone();
        drop(for_the_call);
        let busy = crate::settled::under_way(&crate::settled::watched!(app.core()), crate::settled::PageViews::AtRest);
        drop(for_the_look);
        assert!(busy.iter().any(|what| what.contains("generated contents")), "{busy:?}");
        assert_eq!(app.core().tools.contents_looks().load(std::sync::atomic::Ordering::SeqCst), 0, "none left counted");
    }
}
