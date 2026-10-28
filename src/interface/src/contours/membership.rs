//! Structural discovery only. CSS qualification remains a full ordered read pass.
use std::cmp::Ordering;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CssStyleDeclaration, Document, Element, HtmlElement, MutationRecord, Node, Window};

pub(super) struct Membership {
    ordered: Vec<Member>,
}

/// A registered element and its live computed-style handle. The handle is
/// acquired on the first qualification read and dropped with the entry; every
/// frame still reads current values through it, nothing caches a value.
pub(super) struct Member {
    pub(super) node: HtmlElement,
    style: Option<CssStyleDeclaration>,
}

impl Member {
    fn new(node: HtmlElement) -> Self {
        Self { node, style: None }
    }

    pub(super) fn style(&mut self, view: &Window) -> Result<Option<CssStyleDeclaration>, JsValue> {
        if self.style.is_none() {
            self.style = view.get_computed_style(&self.node)?;
        }
        Ok(self.style.clone())
    }
}

impl Membership {
    pub(super) fn new(document: &Document) -> Result<Self, JsValue> {
        let nodes = document.query_selector_all("*")?;
        let mut ordered = Vec::new();
        for index in 0..nodes.length() {
            let node = nodes
                .item(index)
                .expect("invariant: static selector result index exists")
                .unchecked_into::<Element>();
            if let Some(node) = html(node) {
                ordered.push(Member::new(node));
            }
        }
        Ok(Self { ordered })
    }

    pub(super) fn members(&mut self) -> &mut [Member] {
        &mut self.ordered
    }

    pub(super) fn clear(&mut self) {
        self.ordered.clear();
    }

    pub(super) fn reconcile(&mut self, document: &Document, records: &[MutationRecord]) {
        if records.is_empty() {
            return;
        }
        let changed = js_sys::Set::<HtmlElement>::new_typed();
        let mut pending = Vec::new();
        let mut collect = |node: Element| {
            if let Some(node) = html(node)
                && !changed.has(&node)
            {
                changed.add(&node);
                if document.contains(Some(&node)) {
                    pending.push(node);
                }
            }
        };
        for record in records.iter() {
            for list in [record.added_nodes(), record.removed_nodes()] {
                for index in 0..list.length() {
                    let node = list
                        .item(index)
                        .expect("invariant: mutation node-list index exists");
                    // Adopted iframe nodes keep their original realm prototype.
                    if node.node_type() != Node::ELEMENT_NODE {
                        continue;
                    }
                    let element = node.unchecked_into::<Element>();
                    collect(element.clone());
                    let descendants = element
                        .query_selector_all("*")
                        .expect("invariant: universal element selector is valid");
                    for index in 0..descendants.length() {
                        collect(
                            descendants
                                .item(index)
                                .expect("invariant: static selector result index exists")
                                .unchecked_into(),
                        );
                    }
                }
            }
        }
        if changed.size() == 0 {
            return;
        }
        // Only a structural batch reconciles the ordered vector. Retained
        // elements are not rediscovered; same-document moves rejoin in DOM order.
        self.ordered.retain(|member| !changed.has(&member.node));
        pending.sort_by(document_order);
        let mut retained = std::mem::take(&mut self.ordered).into_iter().peekable();
        let mut ordered = Vec::with_capacity(retained.len() + pending.len());
        for node in pending {
            while retained
                .peek()
                .is_some_and(|old| document_order(&old.node, &node).is_lt())
            {
                ordered.push(
                    retained
                        .next()
                        .expect("invariant: retained element was peeked"),
                );
            }
            ordered.push(Member::new(node));
        }
        ordered.extend(retained);
        self.ordered = ordered;
    }
}

fn html(node: Element) -> Option<HtmlElement> {
    (node.namespace_uri().as_deref() == Some("http://www.w3.org/1999/xhtml"))
        .then(|| node.unchecked_into())
}

fn document_order(left: &HtmlElement, right: &HtmlElement) -> Ordering {
    if left == right {
        Ordering::Equal
    } else if left.compare_document_position(right) & Node::DOCUMENT_POSITION_FOLLOWING != 0 {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}
