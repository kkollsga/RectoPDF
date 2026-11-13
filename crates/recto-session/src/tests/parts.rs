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
    "PartMut",
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

/// **Everything a caller outside the crate could reach of the watched types and of `Session`'s ways to them,** from the crate's own files, outside their
/// test modules, sorted so moving a method doesn't change it: each type's public methods (with `const`, `pub(super)` and the rest said), public fields,
/// derived and implemented traits, the files its `impl` blocks are in, and every `Session` method whose return names a watched type, `Confinement` or a
/// `&mut`.
fn listing() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path).unwrap();
        let code = source.split("#[cfg(test)]\nmod ").next().unwrap_or(&source);
        let mut derives: Vec<String> = Vec::new();
        // The type whose block this is: its name, and whether it's an inherent `impl` (methods) or the struct (fields).
        let mut block: Option<(String, bool)> = None;
        let mut lines = code.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(list) = line.strip_prefix("#[derive(") {
                derives.extend(list.trim_end_matches(")]").split(',').map(|d| d.trim().to_string()));
                continue;
            }
            if line.starts_with("impl") {
                let (tr, ty) = impl_header(line);
                derives.clear();
                match tr {
                    Some(tr) if watched(&ty) => out.push(format!("{ty}: impl {tr}")),
                    Some(_) => {}
                    None => {
                        if watched(&ty) {
                            out.push(format!("{ty}: impl in {file}"));
                        }
                        block = Some((ty, true));
                        continue;
                    }
                }
                block = None;
                continue;
            }
            if let Some(rest) = line.strip_prefix("pub struct ").or_else(|| line.strip_prefix("struct ")) {
                let ty = type_name(rest.split(['{', '(', ';']).next().unwrap_or(rest));
                if watched(&ty) {
                    for derive in derives.drain(..) {
                        out.push(format!("{ty}: derive {derive}"));
                    }
                    if rest.contains("(pub ") || rest.contains("(pub(super)") {
                        out.push(format!("{ty}: pub field 0"));
                    }
                }
                derives.clear();
                block = rest.trim_end().ends_with('{').then_some((ty, false));
                continue;
            }
            if !line.starts_with(' ') && !line.starts_with('#') && !line.starts_with("//") {
                derives.clear();
                if line.starts_with('}') {
                    block = None;
                }
            }
            let Some((ty, methods)) = &block else { continue };
            if *methods {
                if !line.starts_with("    ") || line.starts_with("     ") {
                    continue;
                }
                let Some((vis, rest)) = outward(line) else { continue };
                let name = rest.split(['(', '<']).next().unwrap_or(rest);
                if watched(ty) {
                    out.push(format!("{ty}: {vis} fn {name}"));
                } else if ty == "Session" {
                    // The whole signature, however it's wrapped, for its return type.
                    let mut signature = line.to_string();
                    while !signature.trim_end().ends_with('{') && !signature.trim_end().ends_with(';') {
                        let Some(next) = lines.next() else { break };
                        signature.push(' ');
                        signature.push_str(next.trim());
                    }
                    let ret = signature.split_once(") -> ").map_or("", |(_, ret)| ret.trim_end_matches('{').trim());
                    let reaches = ret.contains("&mut")
                        || ret.contains("Confinement")
                        || WATCHED
                            .iter()
                            .any(|w| ret.split(|c: char| !c.is_alphanumeric() && c != '_').any(|word| word == *w));
                    if reaches {
                        out.push(format!("Session: {vis} fn {name} -> {ret}"));
                    }
                }
            } else if watched(ty)
                && let Some(field) = line.strip_prefix("    pub ").or_else(|| line.strip_prefix("    pub(super) "))
            {
                out.push(format!("{ty}: pub field {}", field.split(':').next().unwrap_or(field)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// **Failure: a component can be replaced, built or reached into from outside the crate, or a method that has to change two components together becomes
/// public on one of them** (R3.5, R3.5a). A public `Default` (derived or written), a `Clone`, a `DerefMut` or a `Deref` on the handle, a `_mut` accessor
/// handing out `&mut Tabs`, or a public field, would let `recto-agent` or `recto-app` write `*session.tabs_mut() = Default::default()` or
/// `std::mem::swap` two sessions' tabs: the confinement cell a quiet run's file wall rests on would go or move (the host's lock-free copy then disagreeing
/// with the session), the owner's unsaved edits would go past the close and discard rules, and components would disagree. A new accessor (`saves_mut`)
/// shows here too. Each line is a choice a reviewer sees: add one only if it can't put a component out of step with another or with the host.
#[test]
fn what_a_component_lets_a_caller_outside_the_crate_do_is_listed() {
    let expected: &[&str] = &[
        "Dormancy: derive Debug",
        "Dormancy: impl in dormancy.rs",
        "Dormancy: pub fn seconds_now",
        "Dormant: impl in dormant.rs",
        "Dormant: pub fn due",
        "Dormant: pub fn may_go",
        "Dormant: pub fn next_at",
        "Dormant: pub fn oldest_first",
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
        "PartMut<Generated>: impl in generated.rs",
        "PartMut<Generated>: pub fn forget",
        "PartMut<Journals>: impl in journals.rs",
        "PartMut<Journals>: pub fn set_history",
        "PartMut<Journals>: pub fn set_journaling",
        "PartMut<Lists>: impl in lists.rs",
        "PartMut<Lists>: pub fn set_agent_at_full",
        "PartMut<PageViewMail>: impl in mail.rs",
        "PartMut<PageViewMail>: pub fn jump_sent",
        "PartMut<PageViewMail>: pub fn note_drawn",
        "PartMut<PageViewMail>: pub fn take_jump",
        "PartMut<PageViewMail>: pub fn take_place_lost",
        "PartMut<Recent>: impl in recent.rs",
        "PartMut<Recent>: pub fn clear",
        "PartMut<Tabs>: impl in registry.rs",
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
        "Session: pub fn generated_mut -> PartMut<'_, generated::Generated>",
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
        "Tabs: pub fn path_of",
        "Tabs: pub fn place_to_show",
        "Tabs: pub fn set_selection",
        "Tabs: pub fn spread_of",
        "Tabs: pub fn tab",
        "Tabs: pub fn tab_notice",
        "Tabs: pub fn touch_at",
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
    let listing = listing();
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(
        listing, expected,
        "what a caller outside the crate can reach of the session's components:\n{listing:#?}"
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
