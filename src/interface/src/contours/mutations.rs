//! Correlate actual output DOM writes with their native child-list records.
//! Object ownership alone never proves who caused a mutation.
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{MutationObserver, MutationRecord, Node};

pub(super) fn records(array: js_sys::Array) -> Vec<MutationRecord> {
    array.iter().map(JsCast::unchecked_into).collect()
}

pub(super) struct PaintBatch {
    pub(super) records: Vec<MutationRecord>,
    pub(super) author_changes: bool,
}

pub(super) struct OutputWrites<'a> {
    // None is used only after controller observation has closed, during restore.
    observer: Option<&'a MutationObserver>,
    batch: PaintBatch,
}

impl<'a> OutputWrites<'a> {
    pub(super) fn new(observer: Option<&'a MutationObserver>) -> Self {
        Self {
            observer,
            batch: PaintBatch {
                records: Vec::new(),
                author_changes: false,
            },
        }
    }

    fn collect_author(&mut self) {
        if let Some(observer) = self.observer {
            let records = records(observer.take_records());
            self.batch.author_changes |= !records.is_empty();
            self.batch.records.extend(records);
        }
    }

    fn finish_write(
        &mut self,
        result: Result<Node, JsValue>,
        mut expected: [Option<Delta>; 2],
    ) -> Result<(), JsValue> {
        match result {
            Ok(_) => {
                if let Some(observer) = self.observer {
                    for record in records(observer.take_records()) {
                        if let Some(index) = expected.iter().position(|delta| {
                            delta.as_ref().is_some_and(|delta| delta.matches(&record))
                        }) {
                            expected[index] = None;
                        } else {
                            // Mixed records and additional synchronous callback
                            // writes remain author changes, even for the same node.
                            self.batch.author_changes = true;
                        }
                        self.batch.records.push(record);
                    }
                }
                Ok(())
            }
            Err(error) => {
                // A host failure does not prove nothing happened. Retain every
                // pending record and the original error for the outer update.
                self.collect_author();
                Err(error)
            }
        }
    }

    pub(super) fn append(&mut self, parent: &Node, node: &Node) -> Result<(), JsValue> {
        self.collect_author();
        let removed = node
            .parent_node()
            .map(|parent| Delta::removed(parent, node));
        let result = parent.append_child(node);
        self.finish_write(result, [removed, Some(Delta::added(parent, node))])
    }

    pub(super) fn insert_before(
        &mut self,
        parent: &Node,
        node: &Node,
        before: Option<&Node>,
    ) -> Result<(), JsValue> {
        self.collect_author();
        let removed = node
            .parent_node()
            .map(|parent| Delta::removed(parent, node));
        let result = parent.insert_before(node, before);
        self.finish_write(result, [removed, Some(Delta::added(parent, node))])
    }

    pub(super) fn remove(&mut self, node: &Node) -> Result<(), JsValue> {
        let Some(parent) = node.parent_node() else {
            return Ok(());
        };
        self.collect_author();
        let result = parent.remove_child(node);
        self.finish_write(result, [Some(Delta::removed(parent, node)), None])
    }

    pub(super) fn finish(mut self) -> PaintBatch {
        // This is the normal end-of-paint discovery drain, including frames
        // with no output writes. Such frames have no extra journal host calls.
        self.collect_author();
        self.batch
    }
}

enum Direction {
    Added,
    Removed,
}
struct Delta {
    target: Node,
    node: Node,
    direction: Direction,
}
impl Delta {
    fn added(parent: &Node, node: &Node) -> Self {
        Self {
            target: parent.clone(),
            node: node.clone(),
            direction: Direction::Added,
        }
    }
    fn removed(parent: Node, node: &Node) -> Self {
        Self {
            target: parent,
            node: node.clone(),
            direction: Direction::Removed,
        }
    }
    fn matches(&self, record: &MutationRecord) -> bool {
        if self.target != record.target() {
            return false;
        }
        let (changed, other) = match self.direction {
            Direction::Added => (record.added_nodes(), record.removed_nodes()),
            Direction::Removed => (record.removed_nodes(), record.added_nodes()),
        };
        changed.length() == 1
            && other.length() == 0
            && changed
                .item(0)
                .expect("invariant: singleton mutation list has its node")
                == self.node
    }
}
