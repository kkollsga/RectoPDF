//! The verdict on each PDF's own contents, with the numbers it goes by, for the evidence in `docs/engineering/generated-contents.md`.
//!
//! `cargo run --release -p recto-core --example contents_verdicts -- FILE...`

use recto_core::NONE;
use recto_core::verdict::judge;
use std::collections::BTreeMap;

fn main() {
    let mut counts: BTreeMap<&'static str, u32> = BTreeMap::new();
    for file in std::env::args().skip(1) {
        let name = std::path::Path::new(&file)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match recto_core::inspect(std::path::Path::new(&file)) {
            Ok(facts) => {
                let toc = &facts.toc;
                let placed = toc.entries().iter().filter(|entry| entry.page != NONE && entry.page < facts.page_count).count();
                let last = toc
                    .entries()
                    .iter()
                    .filter(|entry| entry.page != NONE)
                    .map(|entry| entry.page + 1)
                    .max()
                    .unwrap_or(0);
                let distinct = toc.entries().iter().map(|entry| entry.page).collect::<std::collections::HashSet<_>>().len();
                let verdict = judge(toc, facts.page_count);
                *counts.entry(verdict.code()).or_default() += 1;
                if std::env::var_os("LIST").is_some() {
                    for entry in toc.entries() {
                        let page = if entry.page == NONE { "-".to_string() } else { (entry.page + 1).to_string() };
                        println!("    {}{} .. {page}", "  ".repeat(usize::from(entry.depth)), toc.title(entry));
                    }
                }
                println!(
                    "{:<20} {:<48} pages {:>4} entries {:>4} placed {:>4} distinct {:>4} last {:>4}",
                    verdict.code(),
                    name,
                    facts.page_count,
                    toc.entries().len(),
                    placed,
                    distinct,
                    last
                );
            }
            Err(error) => {
                *counts.entry("unreadable").or_default() += 1;
                println!("{:<20} {name} {error:?}", "unreadable");
            }
        }
    }
    println!("{counts:?}");
}
