//! Maintained typed child-list mutation boundary for DOM observer consumers.
//! Locally authored binding; not a restored upstream generated file.
use super::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = "::js_sys::Object", js_name = "MutationRecord")]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub type MutationRecord;
    #[wasm_bindgen(method, getter, js_class = "MutationRecord")]
    pub fn target(this: &MutationRecord) -> Node;
    #[wasm_bindgen(method, getter, js_class = "MutationRecord", js_name = "addedNodes")]
    pub fn added_nodes(this: &MutationRecord) -> NodeList;
    #[wasm_bindgen(method, getter, js_class = "MutationRecord", js_name = "removedNodes")]
    pub fn removed_nodes(this: &MutationRecord) -> NodeList;
}
