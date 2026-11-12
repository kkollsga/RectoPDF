# recto-agent

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-agent` | **Started** | Typed, versioned commands, permission levels, the JSON envelope. `table.rs` is the one list of commands: the CLI's grammar, `recto help`, the level checks, `SKILL.md` and `REFERENCE.md` all come from it | Hold state of its own |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `find::Hit` | Hit of Find's one query, at most 1,000. No tab holds one | ≤ 64 B plus its words: at most 24 characters before, 64 matched and 64 after |

## Dependencies

| Crate | Why |
|---|---|
| `serde`, `serde_json` | The agent JSON envelope, and the state record, read typed so that a hostile file's lists are capped as they're read. `float_roundtrip` is on, so a place reads back as the same number |
| `grep-regex`, `grep-matcher` | `search --regex`: ripgrep's own engine, run here, so no program is started and nothing like `rg --pre` can run. `grep-regex` brings `regex-automata`, `regex-syntax` and `bstr`. Not `grep-searcher`: it's for reading files by line, where pages are already in memory and patterns cross lines, and it would add `encoding_rs` and `memmap2`. Both MIT or Unlicense. The way it's set up follows `crates/mcp-methods/src/grep/searcher.rs` in the owner's `mcp-methods`, MIT. `recto-agent` only |
