//! **The command table and everything made from it stay what they were** (step R2.2). `recto help` whole, and a fingerprint of every other thing the table makes: each row as data, `recto help <row>`, `recto help <family>`, `recto help --json`, the usage lines, `SKILL.md` and `REFERENCE.md`.
//!
//! Failure caught: moving rows next to their commands reorders the table, drops a row or changes one, so the help an agent reads, the skill it installs or the CLI's grammar changes without anyone meaning it to.
//!
//! `RECTO_BLESS=1` rewrites the golden files, for a change that means to change them.

use recto_agent::{skill, table};

const HELP: &str = include_str!("golden/help.txt");
const MADE: &str = include_str!("golden/table_made.txt");

/// FNV-1a, so a fingerprint is the same on every machine and every run.
fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x} {} bytes", text.len())
}

fn made() -> String {
    let mut out = String::new();
    let mut line = |what: &str, text: &str| out.push_str(&format!("{what}: {}\n", fingerprint(text)));
    for (at, spec) in table::TABLE.iter().enumerate() {
        line(&format!("row {at} {}", spec.name), &format!("{spec:?}"));
        line(&format!("help {}", spec.name), &table::help_for(spec));
        line(&format!("usage {}", spec.name), &table::usage_line(spec));
        line(&format!("by tag {}", spec.tag), table::by_tag(spec.tag).map_or("none", |row| row.name));
    }
    let mut firsts: Vec<&str> = table::TABLE.iter().map(|spec| spec.name.split(' ').next().unwrap_or("")).collect();
    firsts.dedup();
    for first in firsts {
        line(&format!("family {first}"), &table::family_help(&[first.to_string()]).unwrap_or_default());
    }
    line("help --json", &table::table_json().to_string());
    line("SKILL.md", &skill::skill_md());
    line("REFERENCE.md", &skill::reference_md());
    out
}

fn held_to(made: &str, golden: &str, file: &str) {
    if std::env::var_os("RECTO_BLESS").is_some() {
        std::fs::write(format!("{}/tests/golden/{file}", env!("CARGO_MANIFEST_DIR")), made).unwrap();
        return;
    }
    for (number, (now, was)) in made.lines().zip(golden.lines()).enumerate() {
        assert_eq!(now, was, "line {} of tests/golden/{file}", number + 1);
    }
    assert_eq!(made.lines().count(), golden.lines().count(), "the number of lines in tests/golden/{file}");
}

#[test]
fn recto_help_reads_as_it_did() {
    held_to(&table::help_text(), HELP, "help.txt");
}

#[test]
fn everything_made_from_the_table_is_as_it_was() {
    held_to(&made(), MADE, "table_made.txt");
}
