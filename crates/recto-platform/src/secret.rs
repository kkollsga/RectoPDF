//! **A password the person typed for one PDF,** in the one crate both the reader's side (`recto_core`) and the renderer's host (`recto_render_service`) build on, so neither keeps a copy of its own. Re-exported as `recto_core::Password`.

use std::path::Path;

/// **A password the person typed for one PDF.** Held in memory while that PDF is open, so its pages can be drawn, and never written, logged, journaled or put in the state record: it can't be cloned or serialised, its `Debug` says nothing of it, and its bytes are overwritten when it goes. An agent never supplies or reads one.
pub struct Password(Box<[u8]>);

/// The longest password taken. PDF's own limit is 127 bytes; past this is nothing a file could want.
pub const MAX_PASSWORD_BYTES: usize = 1024;

impl Password {
    /// What was typed, if it could be a password: not empty, not too long, and with no NUL, which no PDF password has and the renderer can't take.
    pub fn new(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        (!bytes.is_empty() && bytes.len() <= MAX_PASSWORD_BYTES && !bytes.contains(&0)).then(|| Self(bytes.into()))
    }

    /// [`Self::new`] from a `String` that's handed over, such as the one UniFFI made from the sheet's field: the `String`'s own bytes are overwritten here whatever it held, so the only copy left in Rust is this one.
    pub fn take(typed: String) -> Option<Self> {
        let password = Self::new(&typed);
        wipe(typed.into_bytes());
        password
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Overwrites bytes that held a secret before their memory goes back.
pub fn wipe(mut bytes: Vec<u8>) {
    bytes.fill(0);
    // Handed to `black_box` so the writes aren't dropped as nothing reads them after.
    std::hint::black_box(&bytes);
}

impl std::fmt::Debug for Password {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Password(hidden)")
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Which file this is, wherever it's been moved to on its volume: its device and inode. What a password is kept by, so a file moved while it's open keeps its password, and a new file put in its place doesn't get it. `None` where a file can't be looked at, or has no such numbers.
pub fn file_identity(path: &Path) -> Option<(u64, u64)> {
    crate::file_stamp(path)
        .ok()
        .map(|stamp| (stamp.device, stamp.inode))
        .filter(|identity| *identity != (0, 0))
}

/// The same for a file that's open: the file itself, not whatever's at its path now.
pub fn open_file_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file.metadata().ok().map(|meta| (meta.dev(), meta.ino())).filter(|identity| *identity != (0, 0))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a password can be printed, takes an empty or overlong value or a NUL the renderer would cut at, or `take` leaves the typed `String` whole.
    #[test]
    fn a_password_hides_and_bounds_itself() {
        let password = Password::take("open sesame".to_string()).unwrap();
        assert_eq!(password.bytes(), b"open sesame");
        assert!(!format!("{password:?}").contains("sesame"));
        assert!(Password::new("").is_none() && Password::new("a\0b").is_none() && Password::new(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_none());
        assert!(Password::take(String::new()).is_none());
    }
}
