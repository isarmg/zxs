//! Value-level sharing for large, predominantly immutable metadata records.
//!
//! Cloning shares the ordered map. The first write to a shared map clones all
//! keys and record handles in O(record count), not the record payloads. Mutable
//! access then clones only the selected record when it is still shared. This
//! does not bound history residency or eliminate serialization scans.
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow::Borrow, collections::BTreeMap, ops::Index, sync::Arc};

/// An ordered record map with isolated copy-on-write transaction values.
///
/// Its serialized representation is the ordinary map of records. Shared record
/// handles preserve copy-on-write isolation when a reader outlives the map borrow.
/// Keys are text IDs by default; permanent receipts use typed operation IDs.
#[derive(Debug, PartialEq)]
pub struct SharedRecords<T, K = String> {
    records: Arc<BTreeMap<K, Arc<T>>>,
}

impl<T, K> Clone for SharedRecords<T, K> {
    fn clone(&self) -> Self {
        Self {
            records: Arc::clone(&self.records),
        }
    }
}

impl<T, K: Ord + Clone> Default for SharedRecords<T, K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, K: Ord + Clone> SharedRecords<T, K> {
    pub fn new() -> Self {
        Self {
            records: Arc::new(BTreeMap::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Whether both containers reference the same backing map allocation.
    /// False does not imply different contents. This describes storage identity,
    /// not value immutability for arbitrary T with interior-mutable fields.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.records, &other.records)
    }

    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&T>
    where
        K: Borrow<Q>,
    {
        self.records.get(key).map(Arc::as_ref)
    }

    /// Borrow the stored key and record together without cloning either.
    pub fn get_key_value<Q: Ord + ?Sized>(&self, key: &Q) -> Option<(&K, &T)>
    where
        K: Borrow<Q>,
    {
        self.records
            .get_key_value(key)
            .map(|(key, value)| (key, value.as_ref()))
    }

    /// Share one record across a lock or owner lifetime without copying its payload.
    /// Later writes through this map or `Arc::make_mut` remain isolated.
    pub fn get_shared<Q: Ord + ?Sized>(&self, key: &Q) -> Option<Arc<T>>
    where
        K: Borrow<Q>,
    {
        self.records.get(key).map(Arc::clone)
    }

    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.records.contains_key(key)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &T)> + ExactSizeIterator {
        self.records
            .iter()
            .map(|(key, value)| (key, value.as_ref()))
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator {
        self.records.values().map(Arc::as_ref)
    }

    /// Borrow handles in key order so callers can filter before sharing payloads.
    /// Cloning a selected handle never clones T; ordinary copy-on-write isolation remains.
    pub fn shared_values(&self) -> impl DoubleEndedIterator<Item = &Arc<T>> + ExactSizeIterator {
        self.records.values()
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + ExactSizeIterator {
        self.records.keys()
    }

    /// Merge logical records without materializing incoming or replaced values.
    /// Later mutation remains isolated through the ordinary copy-on-write path.
    pub fn extend_shared(&mut self, source: &Self) {
        if source.is_empty() {
            return;
        }
        Arc::make_mut(&mut self.records).extend(
            source
                .records
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value))),
        );
    }

    /// Remove a record without materializing its value, even when shared.
    pub fn remove_discard<Q: Ord + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            return false;
        }
        Arc::make_mut(&mut self.records).remove(key).is_some()
    }
}

impl<T> SharedRecords<T> {
    /// Select an independent logical subset, sharing immutable record payloads.
    /// Calls the predicate once per record in key order. Full selection shares the backing map;
    /// partial selection copies only selected keys and handles, never T.
    pub fn subset(&self, mut predicate: impl FnMut(&str, &T) -> bool) -> Self {
        let mut remaining = self.records.iter();
        let Some(prefix_len) = remaining.position(|(key, value)| !predicate(key, value.as_ref()))
        else {
            return self.clone();
        };
        Self {
            records: Arc::new(
                self.records
                    .iter()
                    .take(prefix_len)
                    .chain(remaining.filter(|(key, value)| predicate(key, value.as_ref())))
                    .map(|(key, value)| (key.clone(), Arc::clone(value)))
                    .collect(),
            ),
        }
    }
}

impl<T: Clone, K: Ord + Clone> SharedRecords<T, K> {
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut T>
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            return None;
        }
        Arc::make_mut(&mut self.records)
            .get_mut(key)
            .map(Arc::make_mut)
    }

    /// A replaced value is cloned only if another map still references it.
    pub fn insert(&mut self, key: K, value: T) -> Option<T> {
        Arc::make_mut(&mut self.records)
            .insert(key, Arc::new(value))
            .map(Arc::unwrap_or_clone)
    }

    /// Return an owned value, cloning it if another map still references it.
    /// Use `remove_discard` when the caller does not need the removed value.
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<T>
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            return None;
        }
        Arc::make_mut(&mut self.records)
            .remove(key)
            .map(Arc::unwrap_or_clone)
    }
}

impl<T, K: Ord + Clone, Q: Ord + ?Sized> Index<&Q> for SharedRecords<T, K>
where
    K: Borrow<Q>,
{
    type Output = T;
    fn index(&self, key: &Q) -> &Self::Output {
        self.get(key).expect("no entry found for key")
    }
}

impl<T, K: Ord + Clone> Extend<(K, T)> for SharedRecords<T, K> {
    fn extend<I: IntoIterator<Item = (K, T)>>(&mut self, values: I) {
        // Do not use insert: discarded replacements must not clone payloads.
        // An empty extension also leaves the shared map untouched.
        let mut values = values.into_iter();
        let Some(first) = values.next() else {
            return;
        };
        Arc::make_mut(&mut self.records).extend(
            std::iter::once(first)
                .chain(values)
                .map(|(key, value)| (key, Arc::new(value))),
        );
    }
}

impl<T, K: Ord + Clone> FromIterator<(K, T)> for SharedRecords<T, K> {
    fn from_iter<I: IntoIterator<Item = (K, T)>>(values: I) -> Self {
        Self {
            records: Arc::new(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Arc::new(value)))
                    .collect(),
            ),
        }
    }
}

impl<T: Serialize, K: Serialize + Ord + Clone> Serialize for SharedRecords<T, K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter())
    }
}

impl<'de, T: Deserialize<'de>, K: Deserialize<'de> + Ord + Clone> Deserialize<'de>
    for SharedRecords<T, K>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<K, T>::deserialize(deserializer).map(|records| records.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/shared_records.rs"]
mod tests;
