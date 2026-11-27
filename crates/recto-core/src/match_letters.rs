//! **The one spelling of "the same letters"** for matching a block's or a heading's text against a page's characters, shared by `recto-index`'s blocks and `recto-headings`
//! (PDFium and distillPDF don't always agree about ligatures, case or punctuation, so only letters and digits are compared).

/// The letters and digits of `text`, lowercased, with the common ligatures spelled out (`ﬁ` is `fi`). `out` gets each letter with the index of the character it came from,
/// so a run of letters can be turned back into a run of characters.
pub fn match_letters(text: impl IntoIterator<Item = char>, mut out: impl FnMut(char, usize)) {
    for (at, c) in text.into_iter().enumerate() {
        let spelled: &str = match c {
            '\u{FB00}' => "ff",
            '\u{FB01}' => "fi",
            '\u{FB02}' => "fl",
            '\u{FB03}' => "ffi",
            '\u{FB04}' => "ffl",
            '\u{FB05}' | '\u{FB06}' => "st",
            _ => {
                if c.is_alphanumeric() {
                    for lower in c.to_lowercase() {
                        out(lower, at);
                    }
                }
                continue;
            }
        };
        spelled.chars().for_each(|letter| out(letter, at));
    }
}

#[cfg(test)]
mod tests {
    use super::match_letters;

    /// Failure: the two users of a spelling drift apart (a ligature, case or punctuation compared differently), so a heading found by one is missed by the other,
    /// or a letter loses the character it came from, which a block's range is made from.
    #[test]
    fn letters_are_lowercase_alphanumerics_with_ligatures_spelled_out_and_their_origin() {
        let mut got = Vec::new();
        match_letters("A\u{FB01}-1 \u{FB05}!".chars(), |letter, at| got.push((letter, at)));
        assert_eq!(got, [('a', 0), ('f', 1), ('i', 1), ('1', 3), ('s', 5), ('t', 5)]);
    }
}
