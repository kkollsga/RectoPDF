# recto-session

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-session` | **Started** | Tabs, tiers, sidebar and view models: all the rules | Name a UI toolkit |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `TabRecord` | Every tab, including cold | ≤ 256 B plus title |
| `DrawNote` | Recent draw, at most 256 | ≤ 32 B |
| `edit::Entry` | Step of an edited tab's undo history, at most 1,000 in undo and redo together | ≤ 32 B |
