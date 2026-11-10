use super::*;
use data_serialization::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};
use std::marker::PhantomData;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::files::deletion) struct Stored {
    slot: String,
    repository_id: HistoryRepositoryId,
    directory_branch_id: String,
    expected_tree_id: String,
    #[serialization(deserialize_with = "entries")]
    entries: Vec<StoredEntry>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StoredEntry {
    entry_id: String,
    file_id: HistoryFileId,
    path: name_policy::PortablePath,
}

struct Named<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Named<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
            type Value = Named<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a named batch-deletion object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Named)
            }
        }
        deserializer.deserialize_map(Object::<T>(PhantomData))
    }
}

pub(in crate::files::deletion) fn stored<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Stored, D::Error> {
    Named::<Stored>::deserialize(deserializer).map(|Named(stored)| stored)
}
fn entries<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<StoredEntry>, D::Error> {
    Vec::<Named<StoredEntry>>::deserialize(deserializer)
        .map(|entries| entries.into_iter().map(|Named(entry)| entry).collect())
}

impl Stored {
    pub(in crate::files::deletion) fn admit(self) -> Result<Request, api::Error> {
        let Self {
            slot,
            repository_id,
            directory_branch_id,
            expected_tree_id,
            entries,
        } = self;
        Request::new(
            slot,
            repository_id,
            directory_branch_id,
            expected_tree_id,
            entries
                .into_iter()
                .map(|entry| Selected {
                    entry: entry.entry_id,
                    file: entry.file_id,
                    path: entry.path,
                })
                .collect(),
        )
    }
}
impl Request {
    pub(in crate::files::deletion) fn stored(&self) -> Stored {
        Stored {
            slot: self.slot.clone(),
            repository_id: self.repository.clone(),
            directory_branch_id: self.branch.clone(),
            expected_tree_id: self.tree.clone(),
            entries: self
                .entries
                .iter()
                .map(|entry| StoredEntry {
                    entry_id: entry.entry.clone(),
                    file_id: entry.file.clone(),
                    path: entry.path.clone(),
                })
                .collect(),
        }
    }
}
