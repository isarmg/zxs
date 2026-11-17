//! Canonical UTF-8 key spelling length for transaction-local resource accounting.
use crate::{FileNodeId, Id, IdKind};

/// Length of the key's canonical text, excluding JSON quoting or escaping.
/// This measures the retained key spelling, not its platform-dependent Rust layout.
pub trait EncodedLen {
    fn encoded_len(&self) -> usize;
}

impl EncodedLen for String {
    fn encoded_len(&self) -> usize {
        self.len()
    }
}

impl<K: IdKind> EncodedLen for Id<K> {
    fn encoded_len(&self) -> usize {
        K::PREFIX.len() + 1 + 32
    }
}

impl EncodedLen for FileNodeId {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Version(id) => id.encoded_len(),
            Self::Anchor(id) => id.encoded_len(),
            Self::Metadata(id) => id.encoded_len(),
            Self::Merge(id) => id.encoded_len(),
        }
    }
}

macro_rules! text_length {
    ($($name:ty),+ $(,)?) => {
        $(
            impl EncodedLen for $name {
                fn encoded_len(&self) -> usize {
                    self.as_str().len()
                }
            }
        )+
    };
}

text_length!(
    name_policy::Name,
    name_policy::LookupName,
    name_policy::SlotId,
    name_policy::PortablePath,
    name_policy::LookupPath,
);
