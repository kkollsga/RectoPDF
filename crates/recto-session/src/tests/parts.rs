//! What a caller outside the crate can do with each of the session's components, and that nothing outside can build, replace or reach into one.

use super::session;
use crate::edit::Author;
use crate::record::Spread;
use crate::{PartMut, TabId};
use std::path::Path;

/// The components, the looks over them and the handle: whatever is listed of these is what a caller outside the crate can reach.
const WATCHED: &[&str] = &[
    "Tabs",
    "Lists",
    "Journals",
    "PageViewMail",
    "Viewing",
    "Dormancy",
    "Saves",
    "Warm",
    "Generated",
    "Recent",
    "Pages",
    "Dormant",
    "Notices",
    "PartMut",
    "Held",
    "HeldGuard",
    "Comparison",
];

/// The last path segment of a type, without its generic arguments, except the handle's, which says which component it's for: `PartMut<Tabs>`.
fn type_name(ty: &str) -> String {
    let ty = ty.trim().trim_end_matches('{').trim();
    let (base, args) = match ty.find('<') {
        Some(at) => (&ty[..at], ty[at + 1..].trim_end_matches('>')),
        None => (ty, ""),
    };
    let base = base.rsplit("::").next().unwrap_or(base).trim();
    if base != "PartMut" {
        return base.to_string();
    }
    let args: Vec<&str> = args
        .split(',')
        .map(str::trim)
        .filter(|arg| !arg.starts_with('\'') && !arg.is_empty())
        .map(|arg| arg.rsplit("::").next().unwrap_or(arg))
        .collect();
    format!("PartMut<{}>", args.join(", "))
}

/// An `impl` line's trait, if it has one, and its type.
fn impl_header(line: &str) -> (Option<String>, String) {
    let mut rest = line.trim_start_matches("impl");
    if rest.starts_with('<') {
        let mut depth = 0;
        for (at, c) in rest.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                rest = &rest[at + 1..];
                break;
            }
        }
    }
    match rest.split_once(" for ") {
        Some((tr, ty)) => (Some(type_name(tr)), type_name(ty)),
        None => (None, type_name(rest)),
    }
}

fn watched(ty: &str) -> bool {
    let base = ty.split('<').next().unwrap_or(ty);
    WATCHED.contains(&base)
}

/// The visibility a caller outside this module could use: `pub`, `pub const`, `pub(super)` and so on, but not `pub(crate)`, which stays in the crate.
fn outward(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let (vis, rest) = line.split_once(" fn ")?;
    let public = vis == "pub" || vis.starts_with("pub ") || vis.starts_with("pub(super)") || vis.starts_with("pub(in");
    public.then_some((vis, rest))
}

/// The crate's source files, in every folder under `src` but the tests', sorted.
fn source_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "tests") {
                source_files(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The traits an attribute derives: `#[derive(A, B)]`, or `#[cfg_attr(cond, derive(A, B))]`, which derives them in some builds.
fn derived(attribute: &str) -> Vec<String> {
    let Some(at) = attribute.find("derive(") else { return Vec::new() };
    let list = &attribute[at + "derive(".len()..];
    let list = list.split(')').next().unwrap_or(list);
    list.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect()
}

/// What the closures and functions a signature takes are handed `&mut`: the `&mut` arguments of each `Fn(..)`, `FnMut(..)`, `FnOnce(..)` and `fn(..)`
/// in it, a way to reach into a component as sure as returning one (`edit_pages`' shape).
fn handed_mut(signature: &str) -> Vec<String> {
    let mut out = Vec::new();
    for opener in ["FnOnce(", "FnMut(", "Fn(", "fn("] {
        for (at, _) in signature.match_indices(opener) {
            if opener == "fn(" && signature[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let args = &signature[at + opener.len()..];
            let mut depth = 0;
            let end = args
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '(' | '<' => 1,
                        ')' | '>' => -1,
                        _ => 0,
                    };
                    depth < 0
                })
                .map_or(args.len(), |(end, _)| end);
            // The arguments, split at the commas between them and not those inside one's generics.
            let (mut depth, mut start) = (0, 0);
            for (at, c) in args[..end].char_indices().chain([(end, ',')]) {
                match c {
                    '(' | '<' => depth += 1,
                    ')' | '>' => depth -= 1,
                    ',' if depth == 0 => {
                        let arg = args[start..at].trim();
                        if arg.starts_with("&mut") {
                            out.push(arg.to_string());
                        }
                        start = at + 1;
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// Whether a return type reaches a component: a `&mut`, the confinement cell, or a watched type by name.
fn reaches(ret: &str) -> bool {
    ret.contains("&mut")
        || ret.contains("Confinement")
        || WATCHED
            .iter()
            .any(|w| ret.split(|c: char| !c.is_alphanumeric() && c != '_').any(|word| word == *w))
}

/// A function's whole signature, however rustfmt wrapped it, from its first line on.
fn signature<'a>(first: &str, lines: &mut impl Iterator<Item = &'a str>) -> String {
    let mut signature = first.trim().to_string();
    while !signature.ends_with('{') && !signature.ends_with(';') {
        let Some(next) = lines.next() else { break };
        signature.push(' ');
        signature.push_str(next.trim());
    }
    signature.replace("( ", "(").replace(", )", ")").replace(",)", ")")
}

/// **Everything a caller outside the crate could reach of the watched types and of `Session`'s ways to them,** from the crate's own files in every
/// folder, inline modules included and test modules left out, sorted so moving a method doesn't change it: each type's public methods (with `const`,
/// `pub(super)` and the rest said), public fields, derived (`cfg_attr` too) and implemented traits, the files its `impl` blocks are in; every `Session`
/// method whose return names a watched type, `Confinement` or a `&mut`, or which hands a closure `&mut` anything; and every free public function that
/// returns one of those.
fn listing() -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&src, &mut files);
    let mut out = Vec::new();
    for path in files {
        let file = path.strip_prefix(&src).unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path).unwrap();
        let mut derives: Vec<String> = Vec::new();
        // The type whose block this is, whether it's an inherent `impl` (methods) or the struct (fields), and its indent.
        let mut block: Option<(String, bool, usize)> = None;
        // A test module being skipped, by its indent.
        let mut skipping: Option<usize> = None;
        let mut test_next = false;
        let mut lines = source.lines();
        while let Some(line) = lines.next() {
            let code = line.trim_start();
            let indent = line.len() - code.len();
            if let Some(at) = skipping {
                if indent == at && code.starts_with('}') {
                    skipping = None;
                }
                continue;
            }
            if code.starts_with("#[cfg(test)]") {
                test_next = true;
                continue;
            }
            if std::mem::take(&mut test_next) && (code.starts_with("mod ") || code.starts_with("pub mod ")) {
                skipping = (!code.ends_with(';')).then_some(indent);
                continue;
            }
            if code.starts_with("#[derive(") || code.starts_with("#[cfg_attr(") {
                derives.extend(derived(code));
                continue;
            }
            if code.starts_with("impl") {
                let (tr, ty) = impl_header(code);
                let inherent = tr.is_none();
                derives.clear();
                match tr {
                    Some(tr) if watched(&ty) => out.push(format!("{ty}: impl {tr}")),
                    Some(_) => {}
                    None if watched(&ty) => out.push(format!("{ty}: impl in {file}")),
                    None => {}
                }
                block = code.ends_with('{').then_some((ty, inherent, indent));
                continue;
            }
            if let Some(rest) = code.strip_prefix("pub struct ").or_else(|| code.strip_prefix("struct ")) {
                let ty = type_name(rest.split(['{', '(', ';']).next().unwrap_or(rest));
                if watched(&ty) {
                    out.extend(derives.drain(..).map(|derive| format!("{ty}: derive {derive}")));
                    if rest.contains("(pub ") || rest.contains("(pub(super)") {
                        out.push(format!("{ty}: pub field 0"));
                    }
                }
                derives.clear();
                block = rest.trim_end().ends_with('{').then_some((ty, false, indent));
                continue;
            }
            if !code.is_empty() && !code.starts_with('#') && !code.starts_with("//") {
                derives.clear();
            }
            match &block {
                Some((_, _, at)) if indent == *at && code.starts_with('}') => block = None,
                Some((ty, true, at)) if indent == at + 4 => {
                    let Some((vis, rest)) = outward(code) else { continue };
                    let name = rest.split(['(', '<']).next().unwrap_or(rest).to_string();
                    let ty = ty.clone();
                    let signature = signature(code, &mut lines);
                    if watched(&ty) {
                        out.push(format!("{ty}: {vis} fn {name}"));
                    } else if ty == "Session" {
                        let ret = signature.split_once(") -> ").map_or("", |(_, ret)| ret.trim_end_matches('{').trim());
                        if reaches(ret) {
                            out.push(format!("Session: {vis} fn {name} -> {ret}"));
                        }
                        out.extend(handed_mut(&signature).into_iter().map(|arg| format!("Session: {vis} fn {name} hands {arg}")));
                    }
                }
                Some((ty, false, at)) if indent == at + 4 && watched(ty) => {
                    if let Some(field) = code.strip_prefix("pub ").or_else(|| code.strip_prefix("pub(super) ")) {
                        out.push(format!("{ty}: pub field {}", field.split(':').next().unwrap_or(field)));
                    }
                }
                Some(_) => {}
                None => {
                    let Some((vis, rest)) = outward(code) else { continue };
                    let name = rest.split(['(', '<']).next().unwrap_or(rest).to_string();
                    let signature = signature(code, &mut lines);
                    let ret = signature.split_once(") -> ").map_or("", |(_, ret)| ret.trim_end_matches('{').trim());
                    if reaches(ret) {
                        out.push(format!("{file}: {vis} fn {name} -> {ret}"));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// What a caller outside the crate can reach today, one line each, as [`listing`] writes it: see the test below.
const LISTED: &[&str] = &[
    "Dormancy: derive Debug",
    "Dormancy: impl in dormancy.rs",
    "Dormancy: pub fn seconds_now",
    "Dormant: impl in dormant.rs",
    "Dormant: pub fn due",
    "Dormant: pub fn may_go",
    "Dormant: pub fn next_in",
    "Dormant: pub fn oldest_first",
    "Held: impl in held.rs",
    "Held: pub fn confinement",
    "Held: pub fn lock",
    "Held: pub fn new",
    "Held: pub fn try_lock",
    "HeldGuard: impl Deref",
    "HeldGuard: impl DerefMut",
    "HeldGuard: impl Drop",
    "Comparison: derive Clone",
    "Comparison: derive Debug",
    "Comparison: derive Eq",
    "Comparison: derive PartialEq",
    "Comparison: impl in compare.rs",
    "Comparison: pub field sync_from",
    "Comparison: pub field synced",
    "Comparison: pub field tabs",
    "Comparison: pub fn add",
    "Comparison: pub fn check",
    "Comparison: pub fn contains",
    "Comparison: pub fn length_note",
    "Comparison: pub fn new",
    "Comparison: pub fn remove",
    "Comparison: pub fn set_synced",
    "Comparison: pub fn sync_targets",
    "Session: pub fn comparison -> Option<&compare::Comparison>",
    "Session: pub fn edit_pages hands &mut crate::parts::PartMut<'_, PageEdits>",
    "Session: pub fn edit_pages_as hands &mut crate::parts::PartMut<'_, PageEdits>",
    "Generated: derive Debug",
    "Generated: impl in generated.rs",
    "Generated: pub fn making",
    "Generated: pub fn view",
    "Journals: impl in journals.rs",
    "Journals: pub fn has_pending",
    "Journals: pub fn history",
    "Journals: pub fn name_of",
    "Lists: impl in lists.rs",
    "Lists: pub fn edits_record",
    "Lists: pub fn edited_tabs",
    "Lists: pub fn get",
    "Lists: pub fn has_undone_owner_steps",
    "Lists: pub fn holds_owner_edits",
    "Lists: pub fn is_asleep",
    "Lists: pub fn is_edited",
    "Lists: pub fn list_mark",
    "Lists: pub fn needs_rewrite",
    "Lists: pub fn own_pages_needed",
    "Lists: pub fn summary",
    "Lists: pub fn undo_and_redo",
    "PageViewMail: derive Debug",
    "PageViewMail: impl in mail.rs",
    "PageViewMail: pub fn draw_note",
    "PageViewMail: pub fn waiting_jump",
    "Notices: impl in fit.rs",
    "Notices: pub fn fit",
    "Notices: pub fn own_file_expected",
    "Notices: pub fn said",
    "Session: pub fn notices -> Notices<'_>",
    "Pages: impl in pages.rs",
    "Pages: pub fn count",
    "Pages: pub fn for_typed",
    "Pages: pub fn label",
    "Pages: pub fn label_differs",
    "Pages: pub fn place_words",
    "Pages: pub fn position_in_same_file",
    "PartMut<Dormancy>: impl in dormancy.rs",
    "PartMut<Dormancy>: pub fn hold",
    "PartMut<Dormancy>: pub fn set_sleep_files",
    "PartMut<Dormancy>: pub fn take_sleep_read",
    "PartMut<Dormancy>: pub fn take_woken_from_file",
    "PartMut<Journals>: impl in journals.rs",
    "PartMut<Journals>: pub fn set_history",
    "PartMut<Journals>: pub fn set_journaling",
    "PartMut<Lists>: impl in lists.rs",
    "PartMut<Lists>: pub fn set_agent_at_full",
    "PartMut<PageEdits>: impl in editing.rs",
    "PartMut<PageEdits>: pub fn delete",
    "PartMut<PageEdits>: pub fn len",
    "PartMut<PageEdits>: pub fn move_pages",
    "PartMut<PageEdits>: pub fn rotate",
    "PartMut<PageViewMail>: impl in mail.rs",
    "PartMut<PageViewMail>: pub fn jump_sent",
    "PartMut<PageViewMail>: pub fn note_drawn",
    "PartMut<PageViewMail>: pub fn take_jump",
    "PartMut<PageViewMail>: pub fn take_place_lost",
    "PartMut<Recent>: impl in recent.rs",
    "PartMut<Recent>: pub fn clear",
    "PartMut<Tabs>: impl in registry.rs",
    "PartMut<Tabs>: pub fn choose_generated",
    "PartMut<Tabs>: pub fn note_selection",
    "PartMut<Tabs>: pub fn note_password_used",
    "PartMut<Tabs>: pub fn set_spread",
    "PartMut<Tabs>: pub fn set_view_rotation",
    "PartMut<Viewing>: impl in viewing.rs",
    "PartMut<Viewing>: pub fn set_edit_mode",
    "PartMut<Viewing>: pub fn set_find",
    "PartMut<Viewing>: pub fn set_find_current",
    "PartMut<Viewing>: pub fn set_view",
    "Recent: derive Debug",
    "Recent: impl in recent.rs",
    "Recent: pub fn version",
    "Saves: derive Debug",
    "Saves: impl in saves.rs",
    "Saves: pub fn any",
    "Saves: pub fn is_saving",
    "Session: pub fn confinement -> &confined::Confinement",
    "Session: pub fn dormancy -> &dormancy::Dormancy",
    "Session: pub fn dormancy_mut -> PartMut<'_, dormancy::Dormancy>",
    "Session: pub fn dormant -> dormant::Dormant<'_>",
    "Session: pub fn generated -> &generated::Generated",
    "Session: pub fn journals -> &journals::Journals",
    "Session: pub fn journals_mut -> PartMut<'_, journals::Journals>",
    "Session: pub fn lists -> &lists::Lists",
    "Session: pub fn lists_mut -> PartMut<'_, lists::Lists>",
    "Session: pub fn mail -> &mail::PageViewMail",
    "Session: pub fn mail_mut -> PartMut<'_, mail::PageViewMail>",
    "Session: pub fn pages -> pages::Pages<'_>",
    "Session: pub fn recent -> &recent::Recent",
    "Session: pub fn recent_mut -> PartMut<'_, recent::Recent>",
    "Session: pub fn saves -> &saves::Saves",
    "Session: pub fn tabs -> &registry::Tabs",
    "Session: pub fn tabs_mut -> PartMut<'_, registry::Tabs>",
    "Session: pub fn viewing -> &viewing::Viewing",
    "Session: pub fn viewing_mut -> PartMut<'_, viewing::Viewing>",
    "Session: pub fn warm -> &warm::Warm",
    "Tabs: derive Debug",
    "Tabs: impl Deref",
    "Tabs: impl in registry.rs",
    "Tabs: pub fn active",
    "Tabs: pub fn can_go_back_and_forward",
    "Tabs: pub fn for_path",
    "Tabs: pub fn needs_password",
    "Tabs: pub fn own_stamp",
    "Tabs: pub fn own_stamps",
    "Tabs: pub fn path_of",
    "Tabs: pub fn place_to_show",
    "Tabs: pub fn spread_of",
    "Tabs: pub fn tab",
    "Tabs: pub fn tab_notice",
    "Tabs: pub fn trail_of",
    "Tabs: pub fn withheld_from_agents",
    "Viewing: derive Debug",
    "Viewing: impl in viewing.rs",
    "Viewing: pub fn edit_mode",
    "Viewing: pub fn find",
    "Viewing: pub fn view",
    "Warm: derive Debug",
    "Warm: impl in warm.rs",
    "Warm: pub fn holds_read_of",
    "Warm: pub fn ids",
    "Warm: pub fn toc",
];

/// **Failure: a component can be replaced, built or reached into from outside the crate, or a method that has to change two components together becomes
/// public on one of them** (R3.5, R3.5a). A public `Default` (derived or written), a `Clone`, a `DerefMut` or a `Deref` on the handle, a `_mut` accessor
/// handing out `&mut Tabs`, or a public field, would let `recto-agent` or `recto-app` write `*session.tabs_mut() = Default::default()` or
/// `std::mem::swap` two sessions' tabs: the confinement cell a quiet run's file wall rests on would go or move (the host's lock-free copy then disagreeing
/// with the session), the owner's unsaved edits would go past the close and discard rules, and components would disagree. A new accessor (`saves_mut`)
/// shows here too. Each line is a choice a reviewer sees: add one only if it can't put a component out of step with another or with the host.
#[test]
fn what_a_component_lets_a_caller_outside_the_crate_do_is_listed() {
    let listing = listing();
    let mut expected: Vec<String> = LISTED.iter().map(ToString::to_string).collect();
    expected.sort();
    let new: Vec<&String> = listing.iter().filter(|line| !expected.contains(line)).collect();
    let gone: Vec<&String> = expected.iter().filter(|line| !listing.contains(line)).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "what a caller outside the crate can reach of the session's components changed.\nNew, not listed: {new:#?}\nListed, not there: {gone:#?}"
    );
}

/// **Failure (R3.5b): a component added to `Session` isn't added to [`WATCHED`]**, so the listing above never sees what a caller outside the crate can
/// do with it: a public `Default`, `Clone` or field on it, or a `_mut` accessor handing it out, would go by unread. Every field of `Session` but its
/// loader is of a watched type (an `Option` of one counts as the one).
#[test]
fn every_component_of_the_session_is_watched() {
    let lib = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    let start = lib.find("\npub struct Session<").expect("Session is in lib.rs");
    let body = &lib[start..lib[start..].find("\n}").map_or(lib.len(), |end| start + end)];
    let unwatched: Vec<&str> = body
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
        .filter_map(|line| line.split_once(':').map(|(_, ty)| ty.trim().trim_end_matches(',')))
        .map(|ty| ty.strip_prefix("Option<").and_then(|inner| inner.strip_suffix('>')).unwrap_or(ty))
        .map(|ty| ty.rsplit("::").next().unwrap_or(ty))
        .filter(|ty| *ty != "L" && !watched(ty))
        .collect();
    assert!(
        unwatched.is_empty(),
        "these components of Session aren't in WATCHED, so the listing doesn't see them: {unwatched:?}"
    );
}

/// **Failure: the handle a caller outside the crate gets for the tabs can't do what callers there need of it** (set a spread, mark a password used,
/// turn the view), or what it does isn't what the session reads back. The other half, that there's no way from the handle to `Tabs` itself, is held by
/// the listing above: no `Deref`, `DerefMut` or `Clone` on `PartMut`, its field the crate's, and the accessor's return type.
#[test]
fn the_tabs_handle_sets_a_spread_a_turn_and_a_password_and_nothing_else_reaches_the_tabs() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    assert!(s.tabs_mut().set_spread(a, Spread::TwoUp));
    assert!(!s.tabs_mut().set_spread(a, Spread::TwoUp), "the same again changes nothing");
    assert_eq!(s.tabs().spread_of(a), Some(Spread::TwoUp));
    s.tabs_mut().set_view_rotation(a, 5);
    assert_eq!(s.tabs().tab(a).map(|tab| tab.view_rotation), Some(1));
    assert!(!s.tabs().withheld_from_agents(a));
    s.tabs_mut().note_password_used(a);
    assert!(s.tabs().withheld_from_agents(a), "a password used withholds the tab from agents");
    assert!(!s.tabs_mut().set_spread(TabId(999), Spread::TwoUp), "no such tab");
}

/// **Failure: the one setter the lists' handle carries, who the next edits are by, doesn't reach the rule it's for** (an agent at Full may push the
/// owner's steps out of the history), so the handle would be a way in that does nothing, or the wrong thing.
#[test]
fn the_lists_handle_says_only_whether_an_agent_is_at_full() {
    let mut s = session(5);
    assert!(!s.lists.may_push_owner(Author::Agent));
    s.lists_mut().set_agent_at_full(true);
    assert!(s.lists.may_push_owner(Author::Agent));
    assert!(!s.lists.may_push_owner(Author::Owner), "never the person's own edits");
    let _: PartMut<'_, crate::lists::Lists> = s.lists_mut();
}
