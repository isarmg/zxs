//! Lexical policy shared by offline record decoding, path selection and every re-open.
//! Raw Unix bytes only. This does not inspect a path, normalize it or prove ownership.
pub(crate) const MAX_PATH_BYTES: usize = 4096;

#[derive(Copy, Clone)]
pub(crate) enum Kind {
    Directory,
    Regular,
    StorageRoot,
}

impl Kind {
    pub(crate) fn check_path(self, raw: &[u8]) -> Result<(), &'static str> {
        let rest = raw.strip_prefix(b"/").ok_or("expected an absolute path")?;
        if raw.len() > MAX_PATH_BYTES {
            return Err("path exceeds 4096 bytes");
        }
        if rest.is_empty() {
            return if matches!(self, Self::Directory) {
                Ok(())
            } else {
                Err("filesystem root is not an admitted operand of this kind")
            };
        }
        let max_depth = if matches!(self, Self::StorageRoot) {
            256
        } else {
            128
        };
        if rest.split(|b| *b == b'/').enumerate().any(|(i, part)| {
            i >= max_depth || part.is_empty() || part == b"." || part == b".." || part.contains(&0)
        }) {
            return Err("empty, dot, parent, NUL or over-depth path component");
        }
        Ok(())
    }
}

/// Write the same escaped, quoted path as the standard library's path
/// debug format. Using plain `Path::display()` here would hand control
/// characters in user-selected Unix names to the confirmation terminal.
pub(crate) struct QuotedPath<'a>(pub(crate) &'a std::path::Path);

impl std::fmt::Display for QuotedPath<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0, out)
    }
}
