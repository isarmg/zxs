//! PortableName / PortablePath with fixed Unicode 16.0 data.
#![forbid(unsafe_code)]
mod error;
pub use error::Error;
pub type Result<T> = std::result::Result<T, Error>;
use unicode::UnicodeNormalization;
#[doc(hidden)]
pub mod profiling;
mod unicode;
mod unicode16;
/// A validated creation path and its fixed-Unicode lookup key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortablePath {
    display: String,
    key: String,
}
impl PortablePath {
    /// Parse an external creation path, requiring NFC display spelling.
    pub fn parse(path: &str) -> Result<Self> {
        validate_path(path)?;
        Ok(Self {
            display: path.into(),
            key: path_key(path),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.display
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn file_name(&self) -> &str {
        self.display
            .rsplit('/')
            .next()
            .expect("invariant: a portable path has a component")
    }
    /// Ancestors inherit component validity and cached lookup keys.
    pub fn parent(&self) -> Option<Self> {
        let (display, _) = self.display.rsplit_once('/')?;
        let (key, _) = self
            .key
            .rsplit_once('/')
            .expect("invariant: path and key have identical depth");
        Some(Self {
            display: display.into(),
            key: key.into(),
        })
    }
    /// Keep a component suffix without rechecking already parsed names.
    pub fn strip_components(&self, depth: usize) -> Option<Self> {
        let display = component_suffix(&self.display, depth)?;
        let key = component_suffix(&self.key, depth)
            .expect("invariant: path and key have identical depth");
        Some(Self {
            display: display.into(),
            key: key.into(),
        })
    }
    /// Rebase an existing suffix under a validated target; only aggregate budgets change.
    pub fn rebase(&self, depth: usize, target: &Self) -> Result<Self> {
        let suffix = component_suffix(&self.display, depth).ok_or(Error::InvalidPath)?;
        let key_suffix = component_suffix(&self.key, depth)
            .expect("invariant: path and key have identical depth");
        let display = format!("{}/{suffix}", target.as_str());
        validate_path_budget(&display)?;
        Ok(Self {
            display,
            key: format!("{}/{key_suffix}", target.key()),
        })
    }
    pub fn into_string(self) -> String {
        self.display
    }
}

fn component_suffix(path: &str, depth: usize) -> Option<&str> {
    (0..depth).try_fold(path, |path, _| {
        path.split_once('/').map(|(_, suffix)| suffix)
    })
}

/// A validated existing-path reference; canonical-equivalent NFD is permitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupPath {
    display: String,
    key: String,
}
impl LookupPath {
    /// Parse an external lookup reference once, retaining its normalized key.
    pub fn parse(path: &str) -> Result<Self> {
        validate_lookup_path(path)?;
        Ok(Self {
            display: path.into(),
            key: path_key(path),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.display
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    /// Retain a component suffix of this parsed lookup reference without revalidation.
    pub fn strip_components(&self, depth: usize) -> Option<Self> {
        let display = component_suffix(&self.display, depth)?;
        let key = component_suffix(&self.key, depth)
            .expect("invariant: lookup path and key have identical depth");
        Some(Self {
            display: display.into(),
            key: key.into(),
        })
    }
}
impl From<&PortablePath> for LookupPath {
    fn from(path: &PortablePath) -> Self {
        Self {
            display: path.display.clone(),
            key: path.key.clone(),
        }
    }
}
impl From<PortablePath> for LookupPath {
    fn from(path: PortablePath) -> Self {
        Self {
            display: path.display,
            key: path.key,
        }
    }
}
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
    validate_name_inner(name, NameUse::Create)
}
/// Validate an existing name reference without requiring NFC input. New names
/// must still use `validate_name`; character and length restrictions are shared.
pub fn validate_lookup_name(name: &str) -> Result<()> {
    validate_name_inner(name, NameUse::Lookup)
}
#[derive(Clone, Copy)]
enum NameUse {
    Create,
    Lookup,
}
fn validate_name_inner(name: &str, usage: NameUse) -> Result<()> {
    if name.is_empty() || [".", ".."].contains(&name) {
        return Err(Error::EmptyOrDot);
    }
    if name.len() > 240 || name.encode_utf16().count() > 240 {
        return Err(Error::NameTooLong);
    }
    if name.starts_with(' ') || name.ends_with([' ', '.']) {
        return Err(Error::NameEdge);
    }
    let ascii = " !#$%&'()+,-.;=@[]^_`{}~";
    let cjk = "、。，；：？！“”‘’（）【】《》〈〉「」『』〔〕·—…";
    for c in name.chars() {
        let cp = c as u32;
        if in_ranges(cp, unicode16::IGNORED)
            || (!in_ranges(cp, unicode16::ALLOWED) && !ascii.contains(c) && !cjk.contains(c))
        {
            return Err(Error::UnsupportedCharacter(cp));
        }
    }
    // Use the repository's fixed Unicode table even for defensive comparisons;
    // Rust's host-version Unicode uppercase tables must not change this policy.
    let stem = lookup_key(
        name.split_once('.')
            .map_or(name, |(stem, _)| stem)
            .trim_end_matches([' ', '.']),
    );
    let reserved = ["con", "prn", "aux", "nul", "conin$", "conout$", "clock$"];
    let numeric = stem
        .strip_prefix("com")
        .or_else(|| stem.strip_prefix("lpt"))
        .is_some_and(|s| s.chars().count() == 1 && "0123456789¹²³".contains(s));
    if reserved.contains(&stem.as_str()) || numeric {
        return Err(Error::ReservedName);
    }
    if matches!(usage, NameUse::Create) && !unicode::is_nfc(name) {
        return Err(Error::NotNormalized(name.nfc().collect::<String>()));
    }
    Ok(())
}
pub fn validate_path(path: &str) -> Result<()> {
    validate_path_inner(path, NameUse::Create)
}
/// Validate a reference to an existing name, allowing canonical-equivalent
/// spellings. This does not authorize creating or renaming to that spelling.
/// All character, structure and input-length restrictions remain unchanged.
pub fn validate_lookup_path(path: &str) -> Result<()> {
    validate_path_inner(path, NameUse::Lookup)
}
fn validate_path_shape(path: &str) -> Result<()> {
    if path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return Err(Error::InvalidPath);
    }
    validate_path_budget(path)
}
fn validate_path_budget(path: &str) -> Result<()> {
    if path.len() > 768 || path.encode_utf16().count() > 200 {
        return Err(Error::InvalidPath);
    }
    if path.split('/').count() > 32 {
        return Err(Error::PathTooDeep);
    }
    Ok(())
}
fn validate_path_inner(path: &str, usage: NameUse) -> Result<()> {
    validate_path_shape(path)?;
    for component in path.split('/') {
        validate_name_inner(component, usage)?;
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
        return Err(Error::InvalidSlot);
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../test/naming/unit/lib/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/naming/unit/key_tests.rs"]
mod key_tests;

impl std::ops::Deref for PortablePath {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl std::fmt::Display for PortablePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl data_serialization::Serialize for PortablePath {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> data_serialization::Deserialize<'de> for PortablePath {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let raw = <String as data_serialization::Deserialize>::deserialize(deserializer)?;
        Self::parse(&raw).map_err(data_serialization::de::Error::custom)
    }
}

impl PartialEq<str> for PortablePath {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for PortablePath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<String> for PortablePath {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}
