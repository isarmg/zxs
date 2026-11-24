//! Shared ZXS client core. No archive repository, host application, mount, or server crate.
pub mod r3;
pub mod session;

#[cfg(target_os = "linux")]
pub mod bundle;
#[cfg(target_os = "linux")]
mod native;
#[cfg(target_os = "linux")]
pub mod workspace;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) fn invalid(message: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const LETTERS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        text.push(LETTERS[usize::from(b >> 4)].into());
        text.push(LETTERS[usize::from(b & 15)].into());
    }
    text
}
