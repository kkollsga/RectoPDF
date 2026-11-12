//! **The host's records with no feature of their own:** a screen, a highlight's piece and what the host does to its layers. Plain types, restated for UniFFI by
//! the bridge in the module they were in.

use recto_reader::tiles::Op;

/// A screen as the host sees it now.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenInfo {
    pub id: u64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub visible_x: f64,
    pub visible_y: f64,
    pub visible_width: f64,
    pub visible_height: f64,
}

/// A piece of a highlight, in the page's own coordinates, which are its layer's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the host does to its layers, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum ReaderOp {
    Clear,
    /// The document's size, and how far the view may scroll past its top and its end: the rest of the margin at each end.
    Document {
        width: f64,
        height: f64,
        top_room: f64,
        bottom_room: f64,
    },
    Magnify {
        value: f64,
    },
    ScrollTo {
        y: f64,
        seq: u64,
    },
    /// Across, when zoomed in.
    ScrollAcross {
        x: f64,
    },
    /// `width` and `height` are the page before it's turned, and `turns` quarter turns clockwise turn it, with its turned top left at `x` and `y`.
    ShowPage {
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        fit: f64,
        paper_rgb: u32,
        turns: u8,
    },
    HidePage {
        page: u32,
    },
    /// `surface` is an `IOSurfaceRef` the host now owns one reference to, as a number, or zero if `rgb` is the colour.
    Attach {
        id: u64,
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        surface: u64,
        rgb: Option<u32>,
        crop_width: f64,
        crop_height: f64,
        below: bool,
    },
    Detach {
        id: u64,
    },
    /// All of what's selected on a page, in its own coordinates. Nothing clears it.
    Highlight {
        page: u32,
        rects: Vec<HighlightRect>,
    },
    /// The snapshot tool's box on a page, in its own coordinates, or none to take it off.
    SnapshotBox {
        page: u32,
        rect: Option<HighlightRect>,
    },
}

impl From<Op> for ReaderOp {
    fn from(op: Op) -> Self {
        match op {
            Op::Clear => Self::Clear,
            Op::Document {
                width,
                height,
                top_room,
                bottom_room,
            } => Self::Document {
                width,
                height,
                top_room,
                bottom_room,
            },
            Op::Magnify { value } => Self::Magnify { value },
            Op::ScrollTo { y, seq } => Self::ScrollTo { y, seq },
            Op::ScrollAcross { x } => Self::ScrollAcross { x },
            Op::ShowPage {
                page,
                x,
                y,
                width,
                height,
                fit,
                paper_rgb,
                turns,
            } => Self::ShowPage {
                page,
                x,
                y,
                width,
                height,
                fit,
                paper_rgb,
                turns,
            },
            Op::HidePage { page } => Self::HidePage { page },
            Op::Attach {
                id,
                page,
                x,
                y,
                width,
                height,
                surface,
                rgb,
                crop_width,
                crop_height,
                below,
            } => Self::Attach {
                id,
                page,
                x,
                y,
                width,
                height,
                surface,
                rgb,
                crop_width,
                crop_height,
                below,
            },
            Op::Detach { id } => Self::Detach { id },
            Op::Highlight { page, rects } => Self::Highlight {
                page,
                rects: rects
                    .into_iter()
                    .map(|rect| HighlightRect {
                        x: f64::from(rect.x),
                        y: f64::from(rect.y),
                        width: f64::from(rect.width),
                        height: f64::from(rect.height),
                    })
                    .collect(),
            },
        }
    }
}
