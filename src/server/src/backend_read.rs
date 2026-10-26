//! Typed, request-local namespace reads. None of these results grants future admission.
use data_serialization::Serialize;

/// A directory handle names either the synthetic root or a canonical directory.
pub struct ReadDirectory<'a> {
    pub branch: &'a str,
    pub entry_id: &'a str,
    pub path: Option<&'a name_policy::PortablePath>,
    pub selection: DirectorySelection<'a>,
}

pub enum DirectorySelection<'a> {
    Capture,
    Page {
        commit: Option<&'a str>,
        after: Option<&'a str>,
    },
}

/// A bounded page; `more` implies at least one entry. Capture returns an empty page.
#[derive(Debug, Serialize)]
pub struct DirectorySnapshot {
    pub commit_id: String,
    pub entries: Vec<storage_contract::Entry>,
    pub more: bool,
}
