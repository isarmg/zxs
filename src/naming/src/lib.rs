//! PortableName / PortablePath with fixed Unicode 16.0 data.
#![forbid(unsafe_code)]
use storage_contract::{Error, Result};
use unicode::UnicodeNormalization;
#[doc(hidden)]
pub mod profiling;
mod unicode;
mod unicode16;
pub const UNICODE_VERSION: &str = "16.0.0";
pub const NAME_POLICY: &str = "PortableName";
fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    let i = ranges.partition_point(|(_, end)| *end < cp);
    ranges
        .get(i)
        .is_some_and(|(start, end)| (*start..=*end).contains(&cp))
}
pub fn lookup_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len());
    append_lookup_key(&mut key, name);
    key
}
fn append_lookup_key(key: &mut String, name: &str) {
    // ASCII is already NFC and the fixed full-fold table changes only A-Z.
    // ASCII casing is independent of the host's Unicode table version.
    if name.is_ascii() {
        let start = key.len();
        key.push_str(name);
        key[start..].make_ascii_lowercase();
    } else {
        key.extend(unicode::nfc_casefold(name, case_fold));
    }
}
fn case_fold(c: char) -> Option<&'static str> {
    if c.is_ascii() {
        return c.is_ascii_uppercase().then(|| {
            let index = (c as u8 - b'A') as usize;
            &"abcdefghijklmnopqrstuvwxyz"[index..index + 1]
        });
    }
    unicode16::FOLD
        .binary_search_by_key(&(c as u32), |(value, _)| *value)
        .ok()
        .map(|index| unicode16::FOLD[index].1)
}
pub fn validate_name(name: &str) -> Result<()> {
    validate_name_inner(name, true)
}
/// Validate an existing name reference without requiring NFC input. New names
/// must still use `validate_name`; character and length restrictions are shared.
pub fn validate_lookup_name(name: &str) -> Result<()> {
    validate_name_inner(name, false)
}
fn validate_name_inner(name: &str, require_nfc: bool) -> Result<()> {
    let err = |code: storage_contract::ErrorCode, message: &str| Error::new(code, message);
    if name.is_empty() || [".", ".."].contains(&name) {
        return Err(err(
            storage_contract::ErrorCode::NameInvalidCharacter,
            "名称不能为空或点路径",
        ));
    }
    if name.len() > 240 || name.encode_utf16().count() > 240 {
        return Err(err(
            storage_contract::ErrorCode::NameTooLong,
            "名称超过240字节或UTF-16代码单元",
        ));
    }
    if name.starts_with(' ') || name.ends_with([' ', '.']) {
        return Err(err(
            storage_contract::ErrorCode::NameInvalidCharacter,
            "名称不能以空格开头或以空格、句点结尾",
        ));
    }
    let ascii = " !#$%&'()+,-.;=@[]^_`{}~";
    let cjk = "、。，；：？！“”‘’（）【】《》〈〉「」『』〔〕·—…";
    for c in name.chars() {
        let cp = c as u32;
        if in_ranges(cp, unicode16::IGNORED)
            || (!in_ranges(cp, unicode16::ALLOWED) && !ascii.contains(c) && !cjk.contains(c))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::NameInvalidCharacter,
                format!("名称包含不支持的字符 U+{cp:04X}"),
            ));
        }
    }
    // Use the repository's fixed Unicode table even for defensive comparisons;
    // Rust's host-version Unicode uppercase tables must not change this policy.
    let stem = lookup_key(
        name.split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches([' ', '.']),
    );
    let reserved = ["con", "prn", "aux", "nul", "conin$", "conout$", "clock$"];
    let numeric = stem
        .strip_prefix("com")
        .or_else(|| stem.strip_prefix("lpt"))
        .is_some_and(|s| s.chars().count() == 1 && "0123456789¹²³".contains(s));
    if reserved.contains(&stem.as_str()) || numeric {
        return Err(err(
            storage_contract::ErrorCode::NameReserved,
            "名称属于设备保留名",
        ));
    }
    if require_nfc && !unicode::is_nfc(name) {
        return Err(Error::new(
            storage_contract::ErrorCode::NameNotNormalized,
            format!(
                "名称须明确确认为NFC形式：{}",
                name.nfc().collect::<String>()
            ),
        ));
    }
    Ok(())
}
pub fn validate_path(path: &str) -> Result<()> {
    validate_path_inner(path, true)
}
/// Validate a reference to an existing name, allowing canonical-equivalent
/// spellings. This does not authorize creating or renaming to that spelling.
/// All character, structure and input-length restrictions remain unchanged.
pub fn validate_lookup_path(path: &str) -> Result<()> {
    validate_path_inner(path, false)
}
fn validate_path_inner(path: &str, require_nfc: bool) -> Result<()> {
    if path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.len() > 768
        || path.encode_utf16().count() > 200
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PathInvalid,
            "路径不是允许的仓库相对路径或超过长度预算",
        ));
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components.len() > 32 {
        return Err(Error::new(
            storage_contract::ErrorCode::PathTooDeep,
            "目录层级超过32",
        ));
    }
    for c in components {
        validate_name_inner(c, require_nfc)?;
    }
    Ok(())
}
pub fn path_key(path: &str) -> String {
    let mut key = String::with_capacity(path.len());
    for (index, component) in path.split('/').enumerate() {
        // Normalize each component independently, preserving even empty
        // components for callers that have not yet validated the path.
        if index > 0 {
            key.push('/');
        }
        append_lookup_key(&mut key, component);
    }
    key
}
pub fn validate_slot(slot: &str) -> Result<()> {
    if slot.is_empty()
        || slot.len() > 64
        || !slot.as_bytes()[0].is_ascii_alphabetic()
        || !slot
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidSlotId,
            "盘位标识须为字母开头的ASCII字母数字、下划线或连字符，最多64字节",
        ));
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../test/naming/unit/lib/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/naming/unit/key_tests.rs"]
mod key_tests;
