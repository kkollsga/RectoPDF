//! **What an update's hand-off says:** what's in flight that it waits for, the cheap look at whether it could go now, why it isn't going ahead, and how the state's write
//! and read-back went. Plain types the bridge restates for UniFFI as they are.

use recto_control::record_store::FlushReport;

/// How it stands for a hand-off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusyItem {
    pub code: String,
    pub message: String,
}

/// The cheap look at whether an update could go now, from a thread or the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub struct HandOffCheck {
    pub safe: bool,
    pub busy: Vec<BusyItem>,
    /// Since the user last touched the app.
    pub seconds_since_input: f64,
}

/// Why a hand-off isn't going ahead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandOffError {
    /// Something's in flight, and an update should wait.
    Busy { items: Vec<BusyItem> },
    /// The state couldn't be written and confirmed.
    NotSaved { code: String, message: String },
}

impl std::fmt::Display for HandOffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { items } => write!(f, "{}", items.iter().map(|item| item.message.as_str()).collect::<Vec<_>>().join(" ")),
            Self::NotSaved { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for HandOffError {}

/// How the state's write and read-back went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandOffReport {
    pub tabs: u64,
    pub edited_tabs: u64,
    pub record_bytes: u64,
    pub journal_bytes: u64,
    pub wrote_bytes: u64,
    /// How much of the total was making the drive flush.
    pub force_us: u64,
    pub total_us: u64,
}

impl From<FlushReport> for HandOffReport {
    fn from(report: FlushReport) -> Self {
        Self {
            tabs: report.tabs as u64,
            edited_tabs: report.edited_tabs as u64,
            record_bytes: report.record_bytes,
            journal_bytes: report.journal_bytes,
            wrote_bytes: report.wrote_bytes,
            force_us: report.force_us,
            total_us: report.total_us,
        }
    }
}
