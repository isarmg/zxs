//! Only zero-publication scope inputs may enter the genesis material encoder.
use crate::State;

pub(in crate::engine) fn initial_state() -> State {
    State { topology_generation: 1, ..State::default() }
}
