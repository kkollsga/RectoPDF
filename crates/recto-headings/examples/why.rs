//! For one PDF, each heading distillPDF found and what the check for hidden text made of it: whether its letters were found among the page's characters, how many of those had no ink, and how many characters on the page had none.
//!
//! `cargo run --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target --example why -- FILE`

use recto_headings::{Headings, InkSource, PageInk, Scan, find_headings, keep_visible};
use recto_proto::CharRecord;
use recto_render_service::{DocRenderer, Policy, ServiceConfig};
use std::path::PathBuf;

fn main() {
    let file = PathBuf::from(std::env::args().nth(1).unwrap());
    let headings = find_headings(&file, Scan::Full, None, &mut |_, _| {}).unwrap();
    let mut renderer = DocRenderer::new(&file, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
    struct All<'a>(&'a mut DocRenderer);
    impl InkSource for All<'_> {
        fn page(&mut self, page: u32) -> Option<PageInk> {
            let read = self.0.page_characters(page, true, true).ok()?;
            Some(PageInk { chars: read.chars, boxes: read.boxes, frame: read.frame, checked: read.hidden_checked })
        }
    }
    let whole = keep_visible(Headings { pages: headings.pages, found: headings.found.clone(), ..Headings::default() }, &mut All(&mut renderer));
    println!("all at once: kept {} dropped {} unchecked {}", whole.entries.len(), whole.hidden_dropped, whole.pages_not_checked);
    for found in &headings.found {
        let page = renderer.page_characters(found.page, true, true).unwrap();
        let text: String = page.chars.iter().map(|record| if record.has(CharRecord::NO_INK) { record.ch().to_ascii_uppercase() } else { record.ch().to_ascii_lowercase() }).collect();
        let hidden = page.chars.iter().filter(|record| record.has(CharRecord::NO_INK)).count();
        struct One(Option<PageInk>);
        impl InkSource for One {
            fn page(&mut self, _page: u32) -> Option<PageInk> {
                self.0.take()
            }
        }
        let kept = keep_visible(Headings { pages: headings.pages, found: vec![found.clone()], ..Headings::default() }, &mut One(Some(PageInk { chars: page.chars.clone(), boxes: page.boxes.clone(), frame: page.frame, checked: page.hidden_checked }))).entries.len();
        if std::env::var_os("BOXES").is_some() && kept == 0 {
            let first: String = found.title.chars().take(4).collect();
            let text: String = page.chars.iter().map(|record| record.ch()).collect();
            let at = text.find(&first);
            println!("   bbox {:?} frame {:?} first {:?} at {:?} box {:?}", found.bbox, page.frame, first, at, at.and_then(|at| page.boxes.get(text[..at].chars().count())));
        }
        println!("p{} l{} {:?}  page chars {} no-ink {} checked {} kept {kept}", found.page + 1, found.level, found.title, page.chars.len(), hidden, page.hidden_checked);
        if std::env::var_os("SHOW").is_some() {
            println!("   {}", text.replace('\n', " / ").chars().take(600).collect::<String>());
        }
    }
}
