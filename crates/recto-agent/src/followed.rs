//! **What `edits` says of a list that followed its pages into a new version of its file** (the page matcher, slice 2): that it did, how many edits,
//! and onto which version, by the file's size and modification time as the tab knows them. Counts and the file's own stamp only: what a page is (its
//! digest) is never said, to an agent or anyone. Said while the tab says so ([`recto_session::record::FileNotice::EditsFollowed`]), which is until the
//! person leaves the tab; a protected tab's edits never follow, and `edits` refuses one anyway.

use recto_session::record::FileNotice;
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};

/// The `followed` of `edits`: `null`, or how many edits followed and the version they're on.
pub(crate) fn said<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let Some(FileNotice::EditsFollowed { edits }) = session.tabs().tab_notice(id) else {
        return Value::Null;
    };
    json!({
        "edits": edits,
        "onto": session.tabs().own_stamp(id).map(|stamp| json!({ "bytes": stamp.size, "modified_ns": stamp.mtime_ns })),
    })
}

/// Its words, for a person reading `recto edits`.
pub(crate) fn words(data: &Value) -> Option<String> {
    let edits = data["followed"]["edits"].as_u64()?;
    Some(format!(
        "\n  the file was changed by another program, and {edits} unsaved edit{} moved onto the same pages of the new version",
        if edits == 1 { " was" } else { "s were" }
    ))
}
