#![allow(dead_code)]
extern crate self as storage_contract;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    ResourceLimit,
    StateInvalid,
}
#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
}
impl Error {
    pub fn new(code: ErrorCode, _message: &str) -> Self {
        Self { code }
    }
    pub fn code(&self) -> ErrorCode {
        self.code
    }
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Default)]
pub struct State {
    versions: BTreeMap<String, Version>,
    file_path_nodes: BTreeMap<String, Node>,
    file_metadata_nodes: BTreeMap<String, Node>,
    file_branch_merges: BTreeMap<String, Merge>,
    file_node_parents: BTreeMap<String, Vec<String>>,
}
pub struct Version {
    file_id: String,
    history_tree_id: String,
    version_id: String,
    source_version_ids: Vec<String>,
}
pub struct Node {
    file_id: String,
    history_tree_id: String,
    node_id: String,
    parent_node_ids: Vec<String>,
    tags: BTreeSet<String>,
}
pub struct Merge {
    file_id: String,
    history_tree_id: String,
    result_node_id: String,
    parent_node_ids: Vec<String>,
}
#[path = "../../../src/engine/src/business/file_ancestry.rs"]
mod file_ancestry;
