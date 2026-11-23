//! Fresh headless account password policy. Keep this identical in Core and Helper.
pub(crate) const MAX_BYTES: usize = 1024;

pub(crate) fn admissible(password: &str) -> bool {
    (1..=MAX_BYTES).contains(&password.len()) && !password.bytes().any(|b| b.is_ascii_control())
}
