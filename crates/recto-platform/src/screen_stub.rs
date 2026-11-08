//! No window server to ask here. See `screen_mac.rs`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShownWindow {
    pub pid: u32,
    pub number: i64,
    pub frame: [f64; 4],
}

pub fn windows_on_screen() -> Option<Vec<ShownWindow>> {
    None
}
