//! **Printing, as the window has it:** what the print panel needs to know.

/// What the print panel needs to know before it opens, all Rust's to say.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintModel {
    /// The tab the print is of, which the answer goes back with. 0 with nothing showing, and then there's nothing to print.
    pub tab: u32,
    pub pages: u32,
    /// Something's selected, so the panel offers to print just its pages.
    pub has_selection: bool,
    /// The job's name in the print queue: the file's name.
    pub job_title: String,
    /// The first page's size as it's shown, in points, for a print to a PDF at the page's own size.
    pub width_pt: f64,
    pub height_pt: f64,
}
