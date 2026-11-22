// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Bits for the recursive unsigned representation.

use super::sealed::Sealed;

pub trait Bit: Sealed + Copy + Default + 'static {
    const U8: u8;
}

/// The type-level bit 0.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct B0;
/// The type-level bit 1.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct B1;

impl Bit for B0 {
    const U8: u8 = 0;
}

impl Bit for B1 {
    const U8: u8 = 1;
}
