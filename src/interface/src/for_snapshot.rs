//! ZXS snapshot lists combine the Leptos-derived row ownership and Tachys keyed
//! DOM reconciler with per-key reactive payloads. Server snapshots can change a
//! transfer's status without replacing its DOM, focus, or expanded details.
use crate::IntoView;
use reactive_state::{
    owner::Owner,
    signal::{ArcRwSignal, ReadSignal},
    traits::{Set, WithUntracked},
};
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    sync::{Arc, Mutex},
};
use view_macros::component;
use view_renderer::{reactive_state::OwnedView, view::keyed::keyed};

/// Retains keyed row DOM while publishing changed snapshot values to each row.
///
/// Unlike [`crate::control_flow::For`], an existing key receives new data through
/// its read-only signal. Read fields inside reactive closures in `children`.
/// Equal payloads do not notify rows. Removing a key releases its row owner;
/// adding that key again creates a fresh row. Keys must be unique per snapshot.
/// A duplicate key panics before the cache or any row payload is changed.
#[component]
pub fn ForSnapshot<IF, I, T, EF, N, KF, K>(
    /// Current immutable snapshot of the visible rows.
    each: IF,
    /// Stable identity, independent of mutable status or display text.
    key: KF,
    /// Builds a row once and receives its current payload as a read-only signal.
    children: EF,
) -> impl IntoView
where
    IF: Fn() -> I + Send + 'static,
    I: IntoIterator<Item = T> + Send + 'static,
    T: PartialEq + Send + Sync + 'static,
    EF: Fn(ReadSignal<T>) -> N + Send + Clone + 'static,
    N: IntoView + 'static,
    KF: Fn(&T) -> K + Send + 'static,
    K: Clone + Eq + Hash + Send + 'static,
{
    let parent = Owner::current().expect("no reactive owner");
    let rows = Arc::new(SnapshotRows::default());
    let children = move |_, (_, snapshot): (K, ArcRwSignal<T>)| {
        let owner = parent.with(Owner::new);
        let view = owner.with(|| children(snapshot.read_only().into()));
        (drop, OwnedView::new_with_owner(view, owner))
    };
    move || {
        let items = each()
            .into_iter()
            .map(|item| (key(&item), item))
            .collect::<Vec<_>>();
        let items = rows.reconcile(items);
        keyed(items, |(key, _)| key.clone(), children.clone())
    }
}

struct SnapshotRows<K, T> {
    rows: Mutex<HashMap<K, ArcRwSignal<T>>>,
}
impl<K, T> Default for SnapshotRows<K, T> {
    fn default() -> Self {
        Self {
            rows: Mutex::new(HashMap::new()),
        }
    }
}
struct PreparedSnapshot<K, T> {
    items: Vec<(K, ArcRwSignal<T>)>,
    updates: Vec<(ArcRwSignal<T>, T)>,
    removed: HashMap<K, ArcRwSignal<T>>,
}
impl<K: Clone + Eq + Hash, T: PartialEq + Send + Sync + 'static> SnapshotRows<K, T> {
    fn reconcile(&self, items: Vec<(K, T)>) -> Vec<(K, ArcRwSignal<T>)> {
        // Reject duplicate identities before taking the cache lock. Failure
        // cannot partly update visible rows or poison the registry's mutex.
        let mut seen = HashSet::with_capacity(items.len());
        assert!(
            items.iter().all(|(key, _)| seen.insert(key.clone())),
            "ForSnapshot requires unique row keys"
        );
        let mut rows = self.rows.lock().expect("snapshot rows lock poisoned");
        let mut previous = std::mem::take(&mut *rows);
        let mut retained = Vec::with_capacity(items.len());
        let mut updates = Vec::new();
        rows.reserve(items.len());
        for (key, value) in items {
            let signal = match previous.remove(&key) {
                Some(signal) => {
                    updates.push((signal.clone(), value));
                    signal
                }
                None => ArcRwSignal::new(value),
            };
            rows.insert(key.clone(), signal.clone());
            retained.push((key, signal));
        }
        drop(rows);
        // Equality, notification, and removed payload destructors run outside
        // the registry lock; user code may execute during any of these steps.
        PreparedSnapshot {
            items: retained,
            updates,
            removed: previous,
        }
        .publish()
    }
}
impl<K, T: PartialEq + Send + Sync + 'static> PreparedSnapshot<K, T> {
    fn publish(self) -> Vec<(K, ArcRwSignal<T>)> {
        for (signal, value) in self.updates {
            if signal.with_untracked(|current| current != &value) {
                signal.set(value);
            }
        }
        drop(self.removed);
        self.items
    }
}

#[cfg(test)]
#[path = "../../../test/interface/unit/for_snapshot/tests.rs"]
mod tests;
