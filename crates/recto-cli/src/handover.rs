//! `recto update install <zip>`: the hand-over of a development build. This process, which has its caller's file access, opens the zip and puts a copy in the app's own folder; the app then opens only that copy and never a path an agent named (so macOS's question about a removable volume is this process's to wait on, and say so, not the app's). The decisions, limits and words are `recto_update::handover`'s; this only calls them.

use recto_agent::table::Invocation;
use recto_agent::{AgentError, exit};
use recto_control::StatePaths;
use recto_control::client::Client;
use recto_update::handover::{self, Refusal};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

/// The error a refusal comes to, with its words.
fn refused(refusal: &Refusal) -> AgentError {
    let words = refusal.words();
    let mut chars = words.chars();
    let sentence = chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect());
    let kind = if matches!(refusal, Refusal::NotABuild) { exit::USAGE } else { exit::FAILED };
    AgentError::new(refusal.code(), format!("{sentence}."), kind)
        .then("`recto update install <zip>` hands a build over; `recto update status` says how the app stands.")
}

/// Hands the build over and asks the app to install the copy. Returns the app's own envelope. Anything the copy leaves behind is removed again if the app doesn't take it.
pub fn install(paths: &StatePaths, invocation: &Invocation, zip: &Path, wait: Duration) -> Result<Value, AgentError> {
    // Where the app keeps its folder, and whether it answers this user at all: its refusal is the answer.
    let mut client = Client::connect(paths)?;
    let status = client.call(&json!({ "cmd": "update_status" }).to_string(), wait)?;
    if status["ok"] != true {
        return Ok(status);
    }
    let folder = status["data"]["handover_dir"]
        .as_str()
        .map(Path::new)
        .ok_or_else(|| refused(&Refusal::NoFolder))?;
    drop(client);
    let cancel = recto_platform::local::interrupt_flag();
    let copy = handover::bring_in(zip, folder, handover::WAITS, cancel, &mut |words| eprintln!("{words}")).map_err(|refusal| refused(&refusal))?;
    let mut fields = invocation.fields.clone();
    fields.insert("cmd".into(), json!(invocation.spec.tag));
    fields.insert("zip".into(), json!(copy.to_str().unwrap_or_default()));
    // A fresh connection: the copy may have taken a while, and an idle one isn't promised to stay.
    let answer = Client::connect(paths).and_then(|mut client| client.call(&Value::Object(fields).to_string(), wait));
    // The app deletes the copy once it has read it. If it never did (a refusal before it, no answer), nothing stays.
    if answer.as_ref().map_or(true, |envelope| envelope["ok"] != true) {
        handover::discard(&copy, folder);
    }
    answer
}
