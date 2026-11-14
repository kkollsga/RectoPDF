//! **A source's lines outside its test items,** for the scans of the sources that hold a rule over shipping code (a write over a file, a renderer given the
//! tab's set, a session made only by its confining constructors): one copy, here in the test-only crate every crate with such a scan has already, so a
//! scan in any of them reads past a file's first `#[cfg(test)]` the same way (the phase's closing read, 2026-11-15: two scans that cut a file at its first
//! test item read `core/person.rs` and five other files only to a test-only static near their top).

/// A source's lines outside test items, with their index from 0: an item under `#[cfg(test)]` is left out to its end, braces counted as written. An item
/// with no body (a `mod x;`, a field, a variant) ends at its `;` or `,`.
#[must_use]
pub fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut skipping: Option<(bool, i64)> = None;
    for (at, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if skipping.is_none() {
            if trimmed.starts_with("#[cfg(test)]") {
                skipping = Some((false, 0));
                if trimmed.trim_end() == "#[cfg(test)]" {
                    continue;
                }
            } else {
                lines.push((at, line));
                continue;
            }
        }
        let Some((opened, depth)) = skipping.as_mut() else { continue };
        if !*opened && trimmed.starts_with("#[") {
            continue;
        }
        for c in line.chars() {
            match c {
                '{' => {
                    *opened = true;
                    *depth += 1;
                }
                '}' => *depth -= 1,
                _ => {}
            }
        }
        if (*opened && *depth <= 0) || (!*opened && (line.trim_end().ends_with(';') || line.trim_end().ends_with(','))) {
            skipping = None;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::production_lines;

    /// Failure (review of R4.2): a `#[cfg(test)]` on a field or a variant skipped every production line after it up to the next `;`, so a write there went
    /// unseen. Test items are left out to their end, and the production lines after them are read.
    #[test]
    fn a_test_only_field_doesnt_hide_the_lines_after_it() {
        let source = "struct A {\n    #[cfg(test)]\n    seen: u8,\n    kept: u8,\n}\nfn b() {\n    let _ = std::fs::write(x, y);\n}\n#[cfg(test)]\nmod tests {\n    fn c() {}\n}\nfn d() {}\n";
        let read: Vec<&str> = production_lines(source).into_iter().map(|(_, line)| line.trim()).collect();
        assert_eq!(
            read,
            ["struct A {", "kept: u8,", "}", "fn b() {", "let _ = std::fs::write(x, y);", "}", "fn d() {}"]
        );
    }
}
