//! Non-serialized, transaction-local mutation provenance for metadata maps.
//!
//! Each field shares its container until mutation. Cloning is constant-space;
//! the first write to a shared plain map copies that map, not unrelated fields.
//! Provenance is valid only for values without interior mutability, as with
//! State business records: mutation through a shared reference cannot be tracked.
use crate::{EncodedLen, SharedRecords};
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
    value: Arc<T>,
    identity: Arc<()>,
    base: Option<Arc<()>>,
    changes: MapChanges<K>,
    changed_key_bytes: usize,
}
pub type TrackedMap<T, K = String> = Tracked<BTreeMap<K, T>, K>;
pub type TrackedRecords<T, K = String> = Tracked<SharedRecords<T, K>, K>;
pub type TrackedOperations<T> = TrackedRecords<T, crate::OperationId>;
/// File-owned authority with typed incremental dirty keys.
pub type TrackedFiles<T> = TrackedRecords<T, crate::FileId>;

impl<T, K: Clone + Ord> Tracked<T, K> {
    pub fn new(value: T) -> Self {
        Self {
            value: Arc::new(value),
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
    fn key_changed(&mut self, key: &K)
    where
        K: EncodedLen,
    {
        let key_bytes = key.encoded_len();
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
impl<T, K: Clone> Clone for Tracked<T, K> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
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
        self.value.as_ref() == other
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
impl<T: Clone, K: Clone + Ord> DerefMut for Tracked<T, K> {
    fn deref_mut(&mut self) -> &mut T {
        self.all_changed();
        Arc::make_mut(&mut self.value)
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
impl<V, K: Clone + Ord> FromIterator<(K, V)> for TrackedMap<V, K> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}
impl<V, K: Clone + Ord> FromIterator<(K, V)> for TrackedRecords<V, K> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}
impl<V, K: Clone + Ord, const N: usize> From<[(K, V); N]> for TrackedMap<V, K> {
    fn from(values: [(K, V); N]) -> Self {
        values.into_iter().collect()
    }
}
impl<V: Clone, K: Clone + Ord + EncodedLen> TrackedMap<V, K> {
    pub fn entry(&mut self, key: K) -> std::collections::btree_map::Entry<'_, K, V> {
        self.key_changed(&key);
        Arc::make_mut(&mut self.value).entry(key)
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.key_changed(&key);
        Arc::make_mut(&mut self.value).insert(key, value)
    }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        self.key_changed(&owned);
        Arc::make_mut(&mut self.value).get_mut(key)
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        self.key_changed(&owned);
        Arc::make_mut(&mut self.value).remove(key)
    }
    /// Remove selected keys without granting mutable access to retained values.
    /// The predicate scans the map; only removed keys enter the exact write set.
    pub fn retain_keys(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let removed = self
            .value
            .iter()
            .filter(|(key, value)| !keep(key, value))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in removed {
            self.remove(&key);
        }
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        // The predicate receives mutable values, so even retained keys are dirty.
        self.all_changed();
        Arc::make_mut(&mut self.value).retain(|key, value| keep(key, value));
    }
}
impl<V: Clone, K: Clone + Ord + EncodedLen> TrackedRecords<V, K> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.key_changed(&key);
        Arc::make_mut(&mut self.value).insert(key, value)
    }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        self.key_changed(&owned);
        Arc::make_mut(&mut self.value).get_mut(key)
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let owned = self.value.get_key_value(key)?.0.clone();
        self.key_changed(&owned);
        Arc::make_mut(&mut self.value).remove(key)
    }
}
impl<V, K: Clone + Ord + EncodedLen> TrackedRecords<V, K> {
    /// Extend immutable records while recording each incoming key precisely.
    pub fn extend_shared(&mut self, source: &SharedRecords<V, K>) {
        for key in source.keys() {
            self.key_changed(key);
        }
        Arc::make_mut(&mut self.value).extend_shared(source);
    }

    pub fn remove_discard<Q: Ord + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        let Some((owned, _)) = self.value.get_key_value(key) else {
            return false;
        };
        let owned = owned.clone();
        self.key_changed(&owned);
        Arc::make_mut(&mut self.value).remove_discard(key)
    }
}
impl<V: Clone, K: Clone + Ord + EncodedLen> Extend<(K, V)> for TrackedMap<V, K> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, values: I) {
        for (key, value) in values {
            self.insert(key, value);
        }
    }
}
impl<V, K: Clone + Ord + EncodedLen> Extend<(K, V)> for TrackedRecords<V, K> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, values: I) {
        for (key, value) in values {
            self.key_changed(&key);
            Arc::make_mut(&mut self.value).extend(std::iter::once((key, value)));
        }
    }
}
impl<'a, V, K> IntoIterator for &'a TrackedMap<V, K> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::collections::btree_map::Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.value.iter()
    }
}
impl<'a, V: Clone, K: Clone + Ord> IntoIterator for &'a mut TrackedMap<V, K> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = std::collections::btree_map::IterMut<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.all_changed();
        Arc::make_mut(&mut self.value).iter_mut()
    }
}
impl<V: Clone, K: Clone> IntoIterator for TrackedMap<V, K> {
    type Item = (K, V);
    type IntoIter = std::collections::btree_map::IntoIter<K, V>;
    fn into_iter(self) -> Self::IntoIter {
        Arc::unwrap_or_clone(self.value).into_iter()
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/tracked.rs"]
mod tests;
