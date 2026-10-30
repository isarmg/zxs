//! Only the bounded fields consumed by history controls enter the UI model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Content,
    Rename,
    Tags,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParentEndpoint {
    InWindow,
    OutsideWindow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Parent {
    pub(super) id: String,
    pub(super) endpoint: ParentEndpoint,
}
impl Parent {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn endpoint(&self) -> ParentEndpoint {
        self.endpoint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParentSet {
    Complete,
    More { cursor: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileNode {
    pub(super) id: String,
    pub(super) version: Option<String>,
    pub(super) sequence: u64,
    pub(super) kind: NodeKind,
    pub(super) note: String,
    pub(super) body_note: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) path: Option<String>,
    pub(super) previous_path: Option<String>,
    pub(super) parents: Vec<Parent>,
    pub(super) parent_set: ParentSet,
    pub(super) materialized: bool,
    pub(super) inherited: bool,
    pub(super) content_state: Option<storage_contract::ContentState>,
    pub(super) fields: storage_contract::history_window::HistoryNodeFields,
}
impl FileNode {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn version_id(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn field_bytes(
        &self,
        field: storage_contract::history_window::HistoryField,
    ) -> u64 {
        use storage_contract::history_window::HistoryField;
        match field {
            HistoryField::Message => self.fields.message.byte_length,
            HistoryField::Author => self.fields.author.byte_length,
            HistoryField::CreatedAt => self.fields.created_at.byte_length,
        }
    }

    pub(crate) fn kind(&self) -> NodeKind {
        self.kind
    }

    pub(crate) fn note(&self) -> &str {
        &self.note
    }

    pub(crate) fn body_note(&self) -> Option<&str> {
        self.body_note.as_deref()
    }

    pub(crate) fn tags(&self) -> &[String] {
        &self.tags
    }

    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub(crate) fn previous_path(&self) -> Option<&str> {
        self.previous_path.as_deref()
    }

    pub(crate) fn parents(&self) -> &[Parent] {
        &self.parents
    }

    pub(crate) fn parent_set(&self) -> &ParentSet {
        &self.parent_set
    }

    pub(crate) fn visible_content(&self) -> bool {
        self.version.is_some()
    }

    pub(crate) fn materialized(&self) -> bool {
        self.materialized
    }

    pub(crate) fn inherited(&self) -> bool {
        self.inherited
    }

    pub(crate) fn content_state(&self) -> Option<storage_contract::ContentState> {
        self.content_state
    }
}
