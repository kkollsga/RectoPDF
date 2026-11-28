//! **Running heads, footers and page numbers left out of what `get` gives** (`docs/design/blocks.md` S5, `docs/engineering/blocks.md`). Pure: the page's chrome spans come in (from the stored edges or from `recto text`'s own marking, by `chrome::chrome_spans`, the one rule) and what is kept comes out. Nothing here decides what is chrome.
//!
//! - **Offsets stay the page's.** A reference, a cut and `refs.more` are in characters of the page's own text, whatever was left out. Left-out characters take no room against the caller's cap, and a cut never falls inside a left-out stretch: it moves to the stretch's end.
//! - **Unknown is kept.** A page whose chrome isn't known (an index made before edges were kept, a page that failed) is given whole and counted.
//! - **Capped.** A page whose spans add up to more than [`MAX_CHROME_CHARS_LEFT_OUT`] characters is given whole and counted: a hostile page can't make `get` drop its edge rows wholesale.
//! - **What a reference names stays.** A reference that begins inside a stretch of chrome asks for it, so the characters it names are given. One that begins in body text names body text: its 32 characters may run on into a page number, and that stays left out.

/// Most characters one page may have left out as chrome. Over it, the page is given whole.
pub const MAX_CHROME_CHARS_LEFT_OUT: u32 = 1024;

/// The rule for one answer: what the reference names, and the cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The characters the reference names on its own page, `[start, end)`.
    pub named: Option<(u32, u32)>,
    /// Most characters a page may have left out. [`MAX_CHROME_CHARS_LEFT_OUT`], except in a test.
    pub cap: u32,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            named: None,
            cap: MAX_CHROME_CHARS_LEFT_OUT,
        }
    }
}

/// What is known of a page's chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Not known: the page is given whole.
    #[default]
    Unknown,
    Known,
    /// Over the cap: the page is given whole.
    OverCap,
}

/// One page's chrome as an answer leaves it out: ascending, apart and inside the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heads {
    pub state: State,
    spans: Vec<(u32, u32)>,
}

impl Heads {
    /// A page of `chars` characters, with `raw` the spans it is marked with (`None` when they aren't known). `named` is the reference's range if it is on this page.
    #[must_use]
    pub fn of(raw: Option<&[(u32, u32)]>, chars: u32, named: Option<(u32, u32)>, cap: u32) -> Self {
        let Some(raw) = raw else { return Self::default() };
        let mut spans: Vec<(u32, u32)> = raw
            .iter()
            .map(|&(start, end)| (start, end.min(chars)))
            .filter(|(start, end)| start < end)
            .collect();
        spans.sort_unstable();
        let mut apart: Vec<(u32, u32)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            match apart.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => apart.push((start, end)),
            }
        }
        if let Some((from, to)) = named
            && apart.iter().any(|&(start, end)| start <= from && from < end)
        {
            apart = apart
                .into_iter()
                .flat_map(|(start, end)| [(start, end.min(from)), (start.max(to), end)])
                .filter(|(start, end)| start < end)
                .collect();
        }
        let total: u32 = apart.iter().map(|(start, end)| end - start).sum();
        if total > cap {
            return Self {
                state: State::OverCap,
                spans: Vec::new(),
            };
        }
        Self {
            state: State::Known,
            spans: apart,
        }
    }

    /// The characters of `[from, to)` that are left out.
    #[must_use]
    pub fn left_in(&self, from: u32, to: u32) -> u32 {
        self.spans.iter().map(|&(start, end)| end.min(to).saturating_sub(start.max(from))).sum()
    }

    /// The characters of `[from, to)` that are given.
    #[must_use]
    pub fn kept_len(&self, from: u32, to: u32) -> u32 {
        to.saturating_sub(from) - self.left_in(from, to)
    }

    /// `at`, or where the stretch it is inside ends: a cut never falls inside a left-out stretch.
    #[must_use]
    pub fn next_kept(&self, at: u32) -> u32 {
        self.spans.iter().find(|&&(start, end)| start <= at && at < end).map_or(at, |&(_, end)| end)
    }

    /// The first `limit` characters of `[from, to)` that are given, and the page's offset just after the last of them (`to` when all are given).
    #[must_use]
    pub fn copy(&self, chars: &[char], from: u32, to: u32, limit: usize) -> (Vec<char>, u32) {
        let to = to.min(u32::try_from(chars.len()).unwrap_or(u32::MAX));
        let mut out = Vec::with_capacity(limit.min((to.saturating_sub(from)) as usize));
        let mut at = from;
        let mut stretches = self.spans.iter().filter(|&&(_, end)| end > from).peekable();
        while at < to {
            // The run of given characters from `at` to the next left-out stretch, or to `to`.
            let run_end = match stretches.peek() {
                Some(&&(start, end)) if start <= at => {
                    at = end.min(to);
                    stretches.next();
                    continue;
                }
                Some(&&(start, _)) => start.min(to),
                None => to,
            };
            let (run, room) = (run_end - at, u32::try_from(limit - out.len()).unwrap_or(u32::MAX));
            let take = run.min(room);
            out.extend_from_slice(&chars[at as usize..(at + take) as usize]);
            at += take;
            if take < run {
                return (out, at);
            }
        }
        (out, to)
    }

    /// The hidden stretches that fall in `[from, end)`, in characters of what was copied from there (the left-out characters taken out).
    #[must_use]
    pub fn remap(&self, hidden: &[(u32, u32)], from: u32, end: u32) -> Vec<(u32, u32)> {
        hidden
            .iter()
            .filter_map(|&(start, stop)| {
                let (start, stop) = (start.max(from), stop.min(end));
                (start < stop).then(|| (start - from - self.left_in(from, start), stop - from - self.left_in(from, stop)))
            })
            .filter(|(start, stop)| start < stop)
            .collect()
    }
}

/// One cut of a kept range: what [`Heads::take`] gives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cut {
    /// The characters given, none of them left out.
    pub text: Vec<char>,
    /// The hidden stretches inside what was copied, in characters of `text`.
    pub hidden: Vec<(u32, u32)>,
    /// The page's offset just after the last character given (`to` when all were).
    pub end: u32,
    /// Where it goes on: `to` when all were given, else `end` moved past any stretch left out.
    pub cut: u32,
    /// The characters of `[from, cut)` that were left out.
    pub left_out: u32,
}

impl Heads {
    /// The one cut of `[from, to)`: the first `max` characters that are given, taken back to a line end when the range is over `max` ([`super::cut_at`]). `chars` is the page's text and `hidden` its hidden stretches. `get`'s page and hit slice and the paragraph walk both cut by this, so they cut alike.
    #[must_use]
    pub fn take(&self, chars: &[char], hidden: &[(u32, u32)], from: u32, to: u32, max: usize) -> Cut {
        let kept = self.kept_len(from, to) as usize;
        let given = super::cut_at(&self.copy(chars, from, to, max + 1).0, max).min(kept);
        let (text, end) = self.copy(chars, from, to, given);
        let (end, cut) = if given < kept { (end, self.next_kept(end)) } else { (to, to) };
        Cut {
            hidden: self.remap(hidden, from, end),
            left_out: self.left_in(from, cut),
            text,
            end,
            cut,
        }
    }
}

#[cfg(test)]
mod tests;
