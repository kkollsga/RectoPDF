# recto-app

> Part of the engineering notes: [`../README.md`](../README.md).

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-app` | **Started** | The app's own logic with no toolkit and no bridge: the window's view models (`model`, a pure function of its inputs that takes no lock, spawns nothing and calls no page view) and the window's features' rules and state, each in its module, reaching the host through traits it owns (`Clock`, `Listener`), with fakes in its `testkit` feature for tests. Plain types the bridge converts one for one into its records. May depend on what `recto-ffi` depends on, except UniFFI | Name a UI toolkit or UniFFI; depend on `recto-ffi`; enable `testkit` outside `[dev-dependencies]` |
