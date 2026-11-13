//! **What each QA verb takes,** checked here so the host is only ever told what it can do and has no rules of its own about it: a place on a page, a
//! word from a short list, a size, a test's own file, where a shot may go.

use recto_agent::{AgentError, exit};
use std::path::Path;

/// Where a QA mouse verb goes: `PAGE:X,Y`, with `:CLICKS` and `:shift` after it for a press. The page is as `recto` counts them.
#[derive(Debug, PartialEq)]
pub(super) struct PagePlace {
    pub(super) page: u32,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) clicks: u32,
    pub(super) shift: bool,
}

pub(super) fn parse_page_place(value: &str) -> Result<PagePlace, AgentError> {
    let wrong = || {
        bad(format!(
            "`{}` isn't a place. It's `PAGE:X,Y`, with `:CLICKS` and `:shift` after it for a press, such as `3:120,88:2`.",
            cleaned(value)
        ))
    };
    let mut parts = value.split(':');
    let page = parts
        .next()
        .and_then(|word| word.parse::<u32>().ok())
        .and_then(|page| page.checked_sub(1))
        .ok_or_else(wrong)?;
    let (x, y) = parts.next().and_then(|word| word.split_once(',')).ok_or_else(wrong)?;
    let (x, y) = (x.parse::<f64>().map_err(|_| wrong())?, y.parse::<f64>().map_err(|_| wrong())?);
    if !x.is_finite() || !y.is_finite() {
        return Err(wrong());
    }
    let (mut clicks, mut shift) = (1, false);
    for part in parts {
        match part {
            "shift" => shift = true,
            count => clicks = count.parse::<u32>().ok().filter(|count| (1..=4).contains(count)).ok_or_else(wrong)?,
        }
    }
    Ok(PagePlace { page, x, y, clicks, shift })
}

/// Whether a file is one a quiet run's test made: in its state folder, or one of the temp folders a quiet run may use
/// ([`recto_control::quiet::temp_folders`], never what `$TMPDIR` says). Compared by the folders the paths really are, so a link out of one
/// doesn't count.
pub(super) fn is_a_tests_file(path: &std::path::Path, state: Option<&std::path::Path>) -> bool {
    let Ok(file) = path.canonicalize() else { return false };
    state
        .and_then(|state| state.canonicalize().ok())
        .into_iter()
        .chain(recto_control::quiet::temp_folders())
        .any(|root| file.starts_with(root))
}

pub(super) fn check_drive_value(verb: &str, value: &str) -> Result<(), AgentError> {
    let one_of = |words: &[&str]| words.contains(&value);
    let row = || value.parse::<u32>().is_ok();
    let fits = match verb {
        "appearance" => one_of(&["light", "dark"]),
        "window" => one_of(&["open", "close"]),
        "contents_pointer" => one_of(&["in", "out"]),
        "corner_pointer" => one_of(&[
            "top-left:in",
            "top-left:out",
            "top-right:in",
            "top-right:out",
            "lower-right:in",
            "lower-right:out",
        ]),
        "reduce_motion" => one_of(&["on", "off"]),
        "accessibility" => crate::sidebar_access::is_qa_value(value),
        "scroller_style" => one_of(&["legacy", "overlay"]),
        "full_screen" | "minimise" | "hide_app" | "frontmost" => one_of(&["on", "off"]),
        "focus" => one_of(&["page", "sidebar", "find"]),
        "sheet" => one_of(&["open", "closed"]),
        "sidebar_width" => value.parse::<f64>().is_ok_and(f64::is_finite),
        // What shows a window is left out: a quiet run shows none.
        // Save As's sheet is a sheet on the window, as the close prompt is, so a quiet run can show it and answer it.
        "menu" => {
            !matches!(value, "file.open" | "app.about" | "app.quit") && (crate::menus::action(value).is_some() || crate::core::snapshot_tool::is_menu_id(value))
        }
        "panel" => {
            value == "toggle"
                || value == "edit_mode.done"
                || value.starts_with("paper.")
                || value.starts_with("view.")
                || value.starts_with("rotate.")
                || value.starts_with("tools.")
        }
        "button" => one_of(&["sidebar", "zoom", "zoom_reset", "save", "page"]),
        "sheet_answer" => one_of(&["first", "second", "third", "cancel"]),
        "key_action" => one_of(&["undo", "redo"]),
        "context_menu" => crate::menus::context_menu(None, Some(false))
            .iter()
            .any(|item| !item.id.is_empty() && item.id == value),
        // What a person can type in a short field, and no more. Whether it's a level is the field's to find out, as it is for a person.
        "zoom_type" => value.chars().count() <= 32,
        // A test's password, typed as a person would: the app takes it once and keeps it only for the open tab.
        "password" => !value.is_empty() && value.len() <= recto_core::MAX_PASSWORD_BYTES,
        "page_type" => value.len() <= crate::linking::MAX_TYPED_PAGE,
        "swipe" => value.parse::<f64>().is_ok_and(|delta| delta.is_finite() && delta.abs() <= 100.0),
        "hover" | "reveal" => value == "none" || row(),
        "click_row" => value == "note" || value.strip_prefix("fold:").is_some_and(|row| row.parse::<u32>().is_ok()) || row(),
        "contents_scroll" | "compare_click" | "find_click" => row(),
        "palette_type" => value.chars().count() <= 256,
        "palette_key" => one_of(&["up", "down", "return", "escape"]),
        "find_type" => value.len() <= recto_session::record::MAX_FIND_BYTES,
        "find_key" => one_of(&["escape"]),
        "pane_context_menu" => value.split_once(':').is_some_and(|(pane, item)| {
            pane.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES)
                && crate::menus::context_menu(None, Some(false))
                    .iter()
                    .any(|entry| !entry.id.is_empty() && entry.id == item)
        }),
        "pane_focus" => value.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES),
        "pane_scroll" => value.split_once(':').is_some_and(|(pane, points)| {
            pane.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES)
                && points.parse::<f64>().is_ok_and(|points| points.is_finite() && points.abs() <= 1e9)
        }),
        "resize" => value
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?)))
            .is_some_and(|(width, height)| (400.0..=8000.0).contains(&width) && (300.0..=8000.0).contains(&height)),
        _ => false,
    };
    if fits {
        return Ok(());
    }
    Err(bad(format!("`{}` isn't something `{}` takes.", cleaned(value), verb.replace('_', "-"))))
}

/// What a `qa shot` may write: a new `.png`, outside the real home.
pub(super) fn refuse_shot_path(out: &Path, in_real_home: impl Fn(&Path) -> bool) -> Result<(), AgentError> {
    if !out.is_absolute() {
        return Err(bad("That path isn't whole. A session has no folder of its own to start from."));
    }
    if !out.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("png")) {
        return Err(bad("A shot is a new `.png` file."));
    }
    if in_real_home(out) {
        return Err(AgentError::new(
            "path_refused",
            "A shot goes outside your home folder, somewhere temporary.",
            exit::FAILED,
        ));
    }
    // `symlink_metadata`, so a link is something being there, wherever it points or if it points nowhere.
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(recto_agent::output_exists());
    }
    Ok(())
}

pub(super) fn bad(words: impl Into<String>) -> AgentError {
    AgentError::new("usage", words.into(), exit::USAGE)
}

pub(super) fn cleaned(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review of R2.4, S1): a test's own file is decided by the caller's `$TMPDIR`, so a quiet run started with `TMPDIR=$HOME` lets `qa as-person`
    /// and `qa password` act on the person's own files. The wall goes by the system's temp folders, as the quiet run's state rule does, whatever the
    /// environment says. This test binary runs [`tmpdir_is_the_home_here`] again with `TMPDIR` pointed at the home, which can't be set in this process.
    #[test]
    fn a_tests_file_isnt_what_tmpdir_says() {
        let home = recto_platform::local::real_home().expect("the home");
        let child = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", "core::qa::values::tests::tmpdir_is_the_home_here", "--nocapture"])
            .env("TMPDIR", &home)
            .env(TMPDIR_CHILD, "1")
            .output()
            .expect("the test binary again");
        let said = String::from_utf8_lossy(&child.stdout).into_owned() + &String::from_utf8_lossy(&child.stderr);
        assert!(child.status.success() && said.contains("1 passed"), "with TMPDIR at the home:\n{said}");
        let temp = recto_control::quiet::temp_folders();
        assert!(!temp.is_empty(), "no temp folder");
        assert!(is_a_tests_file(&temp[0], None), "the system's temp folder is a test's");
    }

    /// Set when [`a_tests_file_isnt_what_tmpdir_says`] runs this binary again.
    const TMPDIR_CHILD: &str = "RECTO_TEST_TMPDIR_IS_HOME";

    /// Run by [`a_tests_file_isnt_what_tmpdir_says`] with `TMPDIR` at the home; nothing to do otherwise.
    #[test]
    fn tmpdir_is_the_home_here() {
        if std::env::var_os(TMPDIR_CHILD).is_none() {
            return;
        }
        let home = recto_platform::local::real_home().expect("the home").canonicalize().expect("the home is there");
        assert_eq!(std::env::temp_dir().canonicalize().ok(), Some(home.clone()), "TMPDIR is the home here");
        assert!(!is_a_tests_file(&home, None), "the home taken as a test's own because TMPDIR said so");
        assert!(!is_a_tests_file(&home.join("Library"), None), "nor a folder in it");
    }

    /// Failure: a shot lands in the real home, by a path or through a link to it, or takes a document's name. Holds the rule, with what counts as the home given.
    #[test]
    fn a_shot_goes_only_where_it_may() {
        let nowhere = |_: &Path| false;
        let dir = std::env::temp_dir();
        assert_eq!(refuse_shot_path(Path::new("a.png"), nowhere).unwrap_err().code, "usage", "not whole");
        assert_eq!(refuse_shot_path(&dir.join("a.pdf"), nowhere).unwrap_err().code, "usage", "a document's name");
        assert_eq!(refuse_shot_path(&dir.join("a"), nowhere).unwrap_err().code, "usage");
        assert_eq!(refuse_shot_path(&dir.join("a.png"), |_| true).unwrap_err().code, "path_refused");
        assert!(refuse_shot_path(&dir.join("recto-no-such-shot.PNG"), nowhere).is_ok());
    }

    /// Failure: a QA click lands somewhere other than what it says, or a page counted from 0 slips through, or a NaN reaches the host.
    #[test]
    fn a_mouse_verb_says_a_place_or_is_refused() {
        assert_eq!(
            parse_page_place("3:120.5,88").unwrap(),
            PagePlace {
                page: 2,
                x: 120.5,
                y: 88.0,
                clicks: 1,
                shift: false
            }
        );
        assert_eq!(
            parse_page_place("1:0,0:2:shift").unwrap(),
            PagePlace {
                page: 0,
                x: 0.0,
                y: 0.0,
                clicks: 2,
                shift: true
            }
        );
        for wrong in ["", "3", "0:1,1", "3:1", "3:x,1", "3:NaN,1", "3:1,inf", "3:1,1:0", "3:1,1:9", "3:1,1:zzz"] {
            assert_eq!(parse_page_place(wrong).unwrap_err().code, "usage", "{wrong}");
        }
    }

    /// Failure: a word the host can't do reports success: `qa appearance zzz` said it was done and went light, since the rule was the host's. They're checked here and every one is refused.
    #[test]
    fn a_verb_only_takes_what_it_can_do() {
        for (verb, good, wrong) in [
            ("appearance", "dark", "zzz"),
            ("window", "close", "zzz"),
            ("contents_pointer", "in", "zzz"),
            ("reduce_motion", "on", "yes"),
            ("hover", "none", "-1"),
            ("reveal", "3", "x"),
            ("click_row", "0", "none"),
            ("contents_scroll", "9", "1.5"),
            ("resize", "800x600", "10x10"),
        ] {
            assert!(check_drive_value(verb, good).is_ok(), "{verb} {good}");
            assert_eq!(check_drive_value(verb, wrong).unwrap_err().code, "usage", "{verb} {wrong}");
        }
        assert!(check_drive_value("resize", "800").is_err());
    }
}
