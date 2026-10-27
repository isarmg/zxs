//! Non-serialized, transaction-local mutation provenance for metadata maps.
//!
//! This deliberately retains the existing containers. It is a write-set, not
//! a persistent map: cloning a plain BTreeMap still copies its records.
//! Provenance is valid only for values without interior mutability, as with
//! State business records: mutation through a shared reference cannot be tracked.
use crate::SharedRecords;
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
    sync::Arc,
};

const MAX_CHANGED_KEYS: usize = 16_384;
const MAX_CHANGED_KEY_BYTES: usize = 1024 * 1024;

/// A bounded conservative write-set. All is a correctness-preserving fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapChanges<K = String> {
    Keys(Arc<BTreeSet<K>>),
    All,
}
impl<K> MapChanges<K> {
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Keys(keys) if keys.is_empty())
    }
}

#[derive(Debug)]
pub struct Tracked<T, K = String> {
    value: T,
    identity: Arc<()>,
    base: Option<Arc<()>>,
    changes: MapChanges<K>,
    changed_key_bytes: usize,
}
pub type TrackedMap<T> = Tracked<BTreeMap<String, T>>;
pub type TrackedRecords<T> = Tracked<SharedRecords<T>>;
pub type TrackedOperations<T> = Tracked<BTreeMap<crate::OperationId, T>, crate::OperationId>;

impl<T, K: Clone + Ord> Tracked<T, K> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            identity: Arc::new(()),
            base: None,
            changes: MapChanges::All,
            changed_key_bytes: 0,
        }
    }
    /// Start a transaction from this exact logical container identity.
    pub fn begin_tracking(&mut self) {
        self.base = Some(Arc::clone(&self.identity));
        self.changes = MapChanges::Keys(Arc::new(BTreeSet::new()));
        self.changed_key_bytes = 0;
    }
    /// A foreign replacement or lost lineage can never claim an empty delta.
    pub fn changes_since(&self, before: &Self) -> MapChanges<K> {
        if Arc::ptr_eq(&self.identity, &before.identity) {
            MapChanges::Keys(Arc::new(BTreeSet::new()))
        } else if self
            .base
            .as_ref()
            .is_some_and(|base| Arc::ptr_eq(base, &before.identity))
        {
            self.changes.clone()
        } else {
            MapChanges::All
        }
    }
    fn key_changed(&mut self, key: &K, key_bytes: usize) {
        self.identity = Arc::new(());
        if let MapChanges::Keys(keys) = &mut self.changes
            && !keys.contains(key)
        {
            if keys.len() == MAX_CHANGED_KEYS
                || key_bytes > MAX_CHANGED_KEY_BYTES.saturating_sub(self.changed_key_bytes)
            {
                self.changes = MapChanges::All;
            } else {
                self.changed_key_bytes += key_bytes;
                Arc::make_mut(keys).insert(key.clone());
            }
        }
    }
    fn all_changed(&mut self) {
        self.identity = Arc::new(());
        self.changes = MapChanges::All;
    }
}
impl<T: Clone, K: Clone> Clone for Tracked<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            identity: Arc::clone(&self.identity),
            base: self.base.clone(),
            changes: self.changes.clone(),
            changed_key_bytes: self.changed_key_bytes,
        }
    }
}
impl<T: Default, K: Clone + Ord> Default for Tracked<T, K> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
impl<T: PartialEq, K> PartialEq for Tracked<T, K> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T: Eq, K> Eq for Tracked<T, K> {}
impl<T: PartialEq, K> PartialEq<T> for Tracked<T, K> {
    fn eq(&self, other: &T) -> bool {
        &self.value == other
    }
}
impl<T, K> Deref for Tracked<T, K> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
// Compatibility escape hatch: every unknown mutable operation invalidates the
// whole field before yielding its reference. It cannot silently bypass tracking.
impl<T, K: Clone + Ord> DerefMut for Tracked<T, K> {
    fn deref_mut(&mut self) -> &mut T {
        self.all_changed();
        &mut self.value
    }
}
impl<T: Serialize, K> Serialize for Tracked<T, K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>, K: Clone + Ord> Deserialize<'de> for Tracked<T, K> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::new)
    }
}
impl<T, K: Clone + Ord> From<T> for Tracked<T, K> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}
impl<V> FromIterator<(String, V)> for TrackedMap<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}
impl<V> FromIterator<(String, V)> for TrackedRecords<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}
impl<V, const N: usize> From<[(String, V); N]> for TrackedMap<V> {
    fn from(values: [(String, V); N]) -> Self {
        values.into_iter().collect()
    }
}
impl<V> TrackedMap<V> {
    pub fn entry(&mut self, key: String) -> std::collections::btree_map::Entry<'_, String, V> {
        self.key_changed(&key, key.len());
        self.value.entry(key)
    }
    pub fn insert(&mut self, key: String, value: V) -> Option<V> {
        self.key_changed(&key, key.len());
        self.value.insert(key, value)
    }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        String: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        let bytes = owned.len();
        self.key_changed(&owned, bytes);
        self.value.get_mut(key)
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        String: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        let bytes = owned.len();
        self.key_changed(&owned, bytes);
        self.value.remove(key)
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&String, &mut V) -> bool) {
        // The predicate receives mutable values, so even retained keys are dirty.
        self.all_changed();
        self.value.retain(|key, value| keep(key, value));
    }
}
impl<V: Clone> TrackedRecords<V> {
    pub fn insert(&mut self, key: String, value: V) -> Option<V> {
        self.key_changed(&key, key.len());
        self.value.insert(key, value)
    }
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        String: Borrow<Q>,
        Q: Ord + ToOwned<Owned = String> + ?Sized,
    {
        if !self.value.contains_key(key) {
            return None;
        }
        let owned = key.to_owned();
        let bytes = owned.len();
        self.key_changed(&owned, bytes);
        self.value.get_mut(key)
    }
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        String: Borrow<Q>,
        Q: Ord + ToOwned<Owned = String> + ?Sized,
    {
        if !self.value.contains_key(key) {
            return None;
        }
        let owned = key.to_owned();
        let bytes = owned.len();
        self.key_changed(&owned, bytes);
        self.value.remove(key)
    }
}
impl<V> TrackedRecords<V> {
    pub fn remove_discard<Q>(&mut self, key: &Q) -> bool
    where
        String: Borrow<Q>,
        Q: Ord + ToOwned<Owned = String> + ?Sized,
    {
        if !self.value.contains_key(key) {
            return false;
        }
        let owned = key.to_owned();
        let bytes = owned.len();
        self.key_changed(&owned, bytes);
        self.value.remove_discard(key)
    }
}
impl<V> Extend<(String, V)> for TrackedMap<V> {
    fn extend<I: IntoIterator<Item = (String, V)>>(&mut self, values: I) {
        for (key, value) in values {
            self.insert(key, value);
        }
    }
}
impl<V> Extend<(String, V)> for TrackedRecords<V> {
    fn extend<I: IntoIterator<Item = (String, V)>>(&mut self, values: I) {
        for (key, value) in values {
            self.key_changed(&key, key.len());
            self.value.extend(std::iter::once((key, value)));
        }
    }
}
impl<'a, V> IntoIterator for &'a TrackedMap<V> {
    type Item = (&'a String, &'a V);
    type IntoIter = std::collections::btree_map::Iter<'a, String, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.value.iter()
    }
}
impl<'a, V> IntoIterator for &'a mut TrackedMap<V> {
    type Item = (&'a String, &'a mut V);
    type IntoIter = std::collections::btree_map::IterMut<'a, String, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.all_changed();
        self.value.iter_mut()
    }
}
impl<V> IntoIterator for TrackedMap<V> {
    type Item = (String, V);
    type IntoIter = std::collections::btree_map::IntoIter<String, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.value.into_iter()
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/tracked.rs"]
mod tests;

impl<V> TrackedOperations<V> {
    pub fn insert(&mut self, key: crate::OperationId, value: V) -> Option<V> {
        self.key_changed(&key, std::mem::size_of::<crate::OperationId>());
        self.value.insert(key, value)
    }
}
impl<V> FromIterator<(crate::OperationId, V)> for TrackedOperations<V> {
    fn from_iter<I: IntoIterator<Item = (crate::OperationId, V)>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}
