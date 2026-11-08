//! **Links, Back and Forward, and the go-to field: the words and the choices,** for the host to draw. Rust says what a link does and how it's put; the host shows a hint, a hand, and the system's sheet, and opens an address only when this says the person chose to.

use recto_core::Toc;
use recto_core::nav::{LinkTarget, Refused};

/// What hovering over a page shows: the pointer's look, and a line saying where a link goes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PointerLook {
    /// `link`, `ibeam` or `arrow`.
    pub cursor: String,
    /// Where the link under the pointer goes, in words. Nothing off a link.
    pub hint: Option<String>,
}

/// The sheet that asks before a web or email address opens. Owner's brief, 2026-11-08: "External URIs open only after a clear, Rust-worded confirmation that shows the real target."
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LinkPrompt {
    /// Which asking this is, so an answer is to the sheet that was shown and no later one.
    pub id: u64,
    pub title: String,
    pub message: String,
    pub open_title: String,
    pub cancel_title: String,
    /// What opens, exactly as the PDF wrote it and as it's shown.
    pub address: String,
}

/// What the go-to field starts with and says.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PageField {
    /// The page being read, by its label if it has one, to type over.
    pub editing_text: String,
    pub tooltip: String,
}

/// What a link's page is called, for its hint: by its label where the file has one, with its number.
pub(crate) fn page_words(position: u32, own_file_page: Option<u32>, toc: Option<&Toc>) -> String {
    let number = u64::from(position) + 1;
    let label = own_file_page.and_then(|page| toc?.labels.label(page)).filter(|label| *label != number.to_string());
    match label {
        Some(label) => format!("Go to page {label} (page {number})"),
        None => format!("Go to page {number}"),
    }
}

/// The hint for a link, other than one to a page, which [`page_words`] says.
pub(crate) fn hint_for(target: &LinkTarget) -> Option<String> {
    Some(match target {
        LinkTarget::Page { .. } => return None,
        LinkTarget::Web(address) => match address.get(..7) {
            Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => format!("Email {}", &address[7..]),
            _ => format!("Web link: {address}"),
        },
        LinkTarget::History { back: true } => "Go back".to_string(),
        LinkTarget::History { back: false } => "Go forward".to_string(),
        LinkTarget::Refused(why) => refused_hint(*why).to_string(),
        LinkTarget::Nowhere => "This link goes nowhere in this document".to_string(),
    })
}

fn refused_hint(why: Refused) -> &'static str {
    match why {
        Refused::File => "A file or another document: RectoPDF won't open it",
        Refused::Script => "A script: RectoPDF won't run it",
        Refused::Launch => "A program: RectoPDF won't start it",
        Refused::Scheme => "Another app: RectoPDF won't open it",
        Refused::Action => "This link does nothing in RectoPDF",
        Refused::BadAddress => "An address that can't be shown as it is: RectoPDF won't open it",
    }
}

/// An address's real host, past any `user@` a link can put in front to look like another: `https://bank.com@other.net/` goes to `other.net`.
pub(crate) fn host_of(address: &str) -> Option<String> {
    let rest = address.split_once("://")?.1;
    // The authority ends at the first of these, as a browser reads it; a backslash never gets this far, as such an address is refused.
    let authority = rest.split(['/', '?', '#', '\\']).next()?;
    let host = authority.rsplit('@').next()?;
    // Lower-case, as hosts are, so a mix of cases can't make one look like another.
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The sheet for a web or email address, in Rust's words, with the real target shown in full.
pub(crate) fn prompt_for(id: u64, address: &str) -> LinkPrompt {
    let cancel_title = "Cancel".to_string();
    if address.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:")) {
        let to = &address[7..];
        return LinkPrompt { id, title: "Write an email from this link?".into(), message: format!("The PDF links to an email to:\n\n{to}\n\nIt opens your mail app, outside RectoPDF."), open_title: "Open Mail".into(), cancel_title, address: address.to_string() };
    }
    let host = host_of(address).unwrap_or_else(|| address.to_string());
    LinkPrompt { id, title: "Open this link in your browser?".into(), message: format!("The PDF links to a page on {host}:\n\n{address}\n\nIt opens in your browser, outside RectoPDF. Open it only if you trust where it goes."), open_title: "Open Link".into(), cancel_title, address: address.to_string() }
}

/// Which way a swipe goes, as the system has it: a swipe with a positive `delta_x` goes back, as in Safari.
pub(crate) fn swipe_goes_back(delta_x: f64) -> Option<bool> {
    if !delta_x.is_finite() || delta_x == 0.0 {
        return None;
    }
    Some(delta_x > 0.0)
}

/// Longest text the go-to field takes: a label's cap, and room for spaces.
pub(crate) const MAX_TYPED_PAGE: usize = 128;

/// The words when the go-to field names no page.
pub(crate) fn no_such_page(typed: &str) -> String {
    format!("No page is called \u{201C}{}\u{201D}.", recto_core::clean_line(typed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::nav::{LabelRange, LabelStyle, PageLabels};

    /// Failure: a link's hint hides where it really goes: a web address isn't shown in full, an email isn't said to be one, or a refused link reads as though it might work.
    #[test]
    fn a_hint_says_where_a_link_goes() {
        assert_eq!(hint_for(&LinkTarget::Web("https://example.com/a?b".into())).as_deref(), Some("Web link: https://example.com/a?b"));
        assert_eq!(hint_for(&LinkTarget::Web("mailto:a@example.com".into())).as_deref(), Some("Email a@example.com"));
        assert!(hint_for(&LinkTarget::Refused(Refused::Script)).unwrap().contains("won't run"));
        assert_eq!(hint_for(&LinkTarget::History { back: true }).as_deref(), Some("Go back"));
        let mut toc = Toc::default();
        toc.labels = PageLabels::from_ranges(vec![LabelRange { start: 0, first: 1, style: LabelStyle::RomanLower, prefix: "".into() }, LabelRange { start: 4, first: 1, style: LabelStyle::Decimal, prefix: "".into() }], false);
        assert_eq!(page_words(8, Some(8), Some(&toc)), "Go to page 5 (page 9)");
        assert_eq!(page_words(8, None, Some(&toc)), "Go to page 9", "a page added from another file has its number");
        assert_eq!(page_words(2, Some(2), None), "Go to page 3");
    }

    /// Failure: the sheet doesn't show the real host, so `https://bank.com@other.net/` reads as the bank's; or it doesn't show the whole address.
    #[test]
    fn the_prompt_shows_the_real_target() {
        let prompt = prompt_for(1, "https://bank.com@other.net/login");
        assert!(prompt.message.contains("a page on other.net"), "{}", prompt.message);
        assert!(prompt.message.contains("https://bank.com@other.net/login"));
        assert_eq!(prompt.address, "https://bank.com@other.net/login");
        assert_eq!(host_of("http://example.com:8080/x").as_deref(), Some("example.com:8080"), "a port stays, as it's part of where it goes");
        assert_eq!(host_of("https://bank.com#@evil.example/").as_deref(), Some("bank.com"), "past a # is the fragment, not the host");
        assert_eq!(host_of("https://bank.com?@evil.example/").as_deref(), Some("bank.com"), "past a ? is the query");
        assert_eq!(host_of("https://evil.example\\@bank.com/").as_deref(), Some("evil.example"), "a backslash ends the host, as a browser reads it");
        assert_eq!(host_of("https://BaNk.CoM/").as_deref(), Some("bank.com"), "lower-case");
        assert_eq!(recto_core::nav::judge_address(b"https://evil.example\\@bank.com/"), recto_core::nav::LinkTarget::Refused(recto_core::nav::Refused::BadAddress), "and such an address isn't offered at all");
        let mail = prompt_for(2, "mailto:someone@example.com");
        assert!(mail.message.contains("someone@example.com") && mail.open_title == "Open Mail");
    }

    /// Failure: a swipe goes the wrong way, or a swipe of nothing goes anywhere.
    #[test]
    fn a_swipe_goes_as_safari_goes() {
        assert_eq!(swipe_goes_back(1.0), Some(true));
        assert_eq!(swipe_goes_back(-1.0), Some(false));
        assert_eq!(swipe_goes_back(0.0), None);
        assert_eq!(swipe_goes_back(f64::NAN), None);
    }
}
