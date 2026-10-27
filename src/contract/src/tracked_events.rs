//! Exact append provenance for the State event array, without changing its bytes.
//!
//! This still stores a Vec: cloning copies events. Append provenance permits a
//! snapshot encoder to reuse its published prefix; it is not persistent storage.
use crate::Event;
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceChanges {
    /// Only entries at this index and later may differ. An empty suffix is unchanged.
    Append {
        start: usize,
    },
    All,
}

#[derive(Clone, Debug)]
pub struct TrackedEvents {
    values: Vec<Event>,
    identity: Arc<()>,
    base: Option<(Arc<()>, usize)>,
    append_only: bool,
}
impl TrackedEvents {
    pub fn new(values: Vec<Event>) -> Self {
        Self {
            values,
            identity: Arc::new(()),
            base: None,
            append_only: false,
        }
    }
    pub fn begin_tracking(&mut self) {
        self.base = Some((Arc::clone(&self.identity), self.values.len()));
        self.append_only = true;
    }
    pub fn changes_since(&self, before: &Self) -> SequenceChanges {
        if Arc::ptr_eq(&self.identity, &before.identity) {
            SequenceChanges::Append {
                start: before.values.len(),
            }
        } else if self.append_only
            && let Some((base, start)) = &self.base
            && Arc::ptr_eq(base, &before.identity)
        {
            SequenceChanges::Append { start: *start }
        } else {
            SequenceChanges::All
        }
    }
    pub fn push(&mut self, value: Event) {
        self.identity = Arc::new(());
        self.values.push(value);
    }
}
impl Default for TrackedEvents {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
impl PartialEq for TrackedEvents {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}
impl Deref for TrackedEvents {
    type Target = Vec<Event>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl DerefMut for TrackedEvents {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.identity = Arc::new(());
        self.append_only = false;
        &mut self.values
    }
}
impl Extend<Event> for TrackedEvents {
    fn extend<I: IntoIterator<Item = Event>>(&mut self, values: I) {
        let mut values = values.into_iter();
        let Some(first) = values.next() else {
            return;
        };
        self.push(first);
        self.values.extend(values);
    }
}
impl From<Vec<Event>> for TrackedEvents {
    fn from(values: Vec<Event>) -> Self {
        Self::new(values)
    }
}
impl FromIterator<Event> for TrackedEvents {
    fn from_iter<I: IntoIterator<Item = Event>>(values: I) -> Self {
        Self::new(values.into_iter().collect())
    }
}
impl Serialize for TrackedEvents {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.values.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for TrackedEvents {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::deserialize(deserializer).map(Self::new)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/tracked_events.rs"]
mod tests;
