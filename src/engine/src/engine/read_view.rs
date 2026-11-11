use crate::engine::Engine;

/// Counters borrowed from the current owner for the lifetime of the view.
///
/// The owner remains borrowed until the last use of the view:
///
/// ```compile_fail,E0505
/// use storage_engine::Engine;
///
/// fn release_owner(engine: Engine) {
///     let read = engine.read();
///     drop(engine);
///     let _sequence = read.sequence();
/// }
/// ```
///
/// Mutating the owner requires the shared borrow to end first:
///
/// ```compile_fail,E0502
/// use storage_engine::Engine;
///
/// fn change_owner(_: &mut Engine) {}
/// fn hold_read(engine: &mut Engine) {
///     let read = engine.read();
///     change_owner(engine);
///     let _sequence = read.sequence();
/// }
/// ```
pub struct ReadView<'a> {
    sequence: &'a u64,
    topology_generation: &'a u64,
}

impl<'a> ReadView<'a> {
    pub(in crate::engine) fn new(sequence: &'a u64, topology_generation: &'a u64) -> Self {
        Self {
            sequence,
            topology_generation,
        }
    }

    /// Sequence of the owner's current published state.
    pub fn sequence(&self) -> u64 {
        *self.sequence
    }

    /// Topology generation of the owner's current published state.
    pub fn topology_generation(&self) -> u64 {
        *self.topology_generation
    }
}

impl Engine {
    /// Borrow the current published counters in constant time.
    pub fn read(&self) -> ReadView<'_> {
        ReadView::new(&self.state.sequence, &self.state.topology_generation)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/read_view_tests.rs"]
mod tests;
