# recto-app

> Part of the engineering notes: [`../README.md`](../README.md).

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-app` | **Started** | The app's own logic with no toolkit and no bridge: the window's view models, projected from the session and a few plain facts, with no lock, thread, page view or listener inside. Plain types the bridge converts one for one into its records. May depend on `recto-session`, `recto-core`, `recto-platform` and `recto-reader` (for the page map and the paper, which are data) | Name a UI toolkit or UniFFI; depend on `recto-ffi`, `recto-agent` or `recto-control`; take a lock, spawn a thread or call a page view |
