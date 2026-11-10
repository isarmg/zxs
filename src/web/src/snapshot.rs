//! Cheap immutable-data projections. Equality is allocation identity, so a
//! reactive memo does not traverse a JSON tree on unrelated State updates.
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Debug)]
pub(crate) struct Snapshot<T>(Arc<T>);
impl<T> Clone for Snapshot<T> {
    fn clone(&self) -> Self {
        #[cfg(test)]
        CLONES.with(|count| count.set(count.get() + 1));
        Self(self.0.clone())
    }
}
impl<T> From<T> for Snapshot<T> {
    fn from(value: T) -> Self {
        Self(Arc::new(value))
    }
}
impl<T: Default> Default for Snapshot<T> {
    fn default() -> Self {
        T::default().into()
    }
}
impl<T> PartialEq for Snapshot<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T> Eq for Snapshot<T> {}
impl<T> Deref for Snapshot<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone> DerefMut for Snapshot<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}
#[cfg(test)]
thread_local! { pub(crate) static CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
