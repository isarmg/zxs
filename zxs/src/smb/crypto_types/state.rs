// Adapted by ZXS from RustCrypto (MIT OR Apache-2.0); see ORIGIN.md.

use core::fmt;
use crate::smb::fixed_array::{Array, ArraySize};

pub type SerializedState<T> = Array<u8, <T as SerializableState>::SerializedStateSize>;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct DeserializeStateError;

impl fmt::Display for DeserializeStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("Deserialization error")
    }
}

impl core::error::Error for DeserializeStateError {}

/// An internal state snapshot may contain sensitive bytes.
pub trait SerializableState
where
    Self: Sized,
{
    type SerializedStateSize: ArraySize;

    fn serialize(&self) -> SerializedState<Self>;

    /// # Errors
    /// Returns `DeserializeStateError` if the state cannot be decoded.
    fn deserialize(serialized_state: &SerializedState<Self>)
    -> Result<Self, DeserializeStateError>;
}
