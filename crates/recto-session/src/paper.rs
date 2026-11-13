//! The paper a page is shown on, and the contrast it keeps with text.

use crate::record::PaperName;

/// The paper a page is shown on, which has nothing to do with the window's own light or dark. Owner, 2026-11-07: "We have two paper color modes: white and dark, let's add a yellowed white option that's better for the eyes".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    White,
    Warm,
    Dark,
}

/// Warm paper's colour, `0xRRGGBB`. Soft cream, as aged book paper is: warm at a glance without reading as yellow. Taken towards the eye from the first idea of `#F5EEDC`, as a multiply over a page leaves black black, and it holds well over WCAG AA for body text: see the test.
pub const WARM_PAPER: u32 = 0xF5_EE_DC;

impl Paper {
    pub(crate) fn to_record(self) -> PaperName {
        match self {
            Self::White => PaperName::White,
            Self::Warm => PaperName::Warm,
            Self::Dark => PaperName::Dark,
        }
    }

    pub(crate) fn from_record(name: PaperName) -> Self {
        match name {
            PaperName::White => Self::White,
            PaperName::Warm => Self::Warm,
            PaperName::Dark => Self::Dark,
        }
    }

    /// Every paper, in the order they're offered.
    pub const ALL: [Self; 3] = [Self::White, Self::Warm, Self::Dark];

    pub fn name(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Warm => "warm",
            Self::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|paper| paper.name() == name)
    }

    /// What it's called on a menu, when that exists.
    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Warm => "Warm",
            Self::Dark => "Dark",
        }
    }

    /// With nothing chosen, the paper goes with the window.
    pub fn resolved(chosen: Option<Self>, window_is_dark: bool) -> Self {
        chosen.unwrap_or(if window_is_dark { Self::Dark } else { Self::White })
    }
}

/// WCAG's contrast ratio between two colours, `0xRRGGBB`: from 1 to 21. AA for body text is 4.5.
pub fn contrast_ratio(one: u32, two: u32) -> f64 {
    fn luminance(rgb: u32) -> f64 {
        let channel = |shift: u32| {
            let value = f64::from((rgb >> shift) & 0xFF) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let (one, two) = (luminance(one), luminance(two));
    (one.max(two) + 0.05) / (one.min(two) + 0.05)
}
