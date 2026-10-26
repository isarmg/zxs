//! Identity-based resize registration; geometry remains a full-frame measurement.
use std::cell::Cell;
use wasm_bindgen::JsCast;
use web_sys::{Element, MutationRecord, Node, ResizeObserver};

pub(super) struct Nodes {
    root: Element,
    resize: ResizeObserver,
    registered: js_sys::Set<Element>,
    closed: Cell<bool>,
}

impl Nodes {
    pub(super) fn new(root: Element, resize: ResizeObserver) -> Self {
        let nodes = Self {
            root,
            resize,
            registered: js_sys::Set::new_typed(),
            closed: Cell::new(false),
        };
        nodes.visit(&nodes.root, ".zxs-version-node", |element| {
            nodes.admit(element)
        });
        nodes
    }

    fn visit(&self, node: &Node, selector: &str, mut action: impl FnMut(Element)) {
        let Some(element) = node.dyn_ref::<Element>() else {
            return;
        };
        if element
            .matches(selector)
            .expect("invariant: version-node selector is valid")
        {
            action(element.clone());
        }
        let descendants = element
            .query_selector_all(selector)
            .expect("invariant: version-node selector is valid");
        for index in 0..descendants.length() {
            action(
                descendants
                    .item(index)
                    .expect("invariant: static selector result index exists")
                    .unchecked_into(),
            );
        }
    }

    fn admit(&self, element: Element) {
        if self.root.contains(Some(&element)) && !self.registered.has(&element) {
            self.registered.add(&element);
            self.resize.observe(&element);
        }
    }

    pub(super) fn reconcile(&self, records: js_sys::Array, links: &Element) -> bool {
        if self.closed.get() {
            return false;
        }
        let mut dirty = false;
        for record in records.iter() {
            let record: MutationRecord = record.unchecked_into();
            if links.contains(Some(&record.target())) {
                continue;
            }
            let added = record.added_nodes();
            for index in 0..added.length() {
                let node = added
                    .item(index)
                    .expect("invariant: mutation node-list index exists");
                // Only the actual connector identity is output. Mixed records
                // still reconcile every real subtree and schedule measurement.
                if node.is_same_node(Some(links)) {
                    continue;
                }
                dirty = true;
                self.visit(&node, ".zxs-version-node", |element| self.admit(element));
            }
            let removed = record.removed_nodes();
            for index in 0..removed.length() {
                let node = removed
                    .item(index)
                    .expect("invariant: mutation node-list index exists");
                if node.is_same_node(Some(links)) {
                    continue;
                }
                dirty = true;
                // Removal is by registered identity, even after a class change.
                // Same-root moves retain their registration at delivery time.
                self.visit(&node, "*", |element| {
                    if !self.root.contains(Some(&element)) && self.registered.delete(&element) {
                        self.resize.unobserve(&element);
                    }
                });
            }
        }
        dirty
    }

    pub(super) fn close(&self) {
        self.closed.set(true);
        self.registered.clear();
    }
}
