//! Freeze every original pending database pair under the same move candidate.
//! This is a preparation identity, not a decision, receipt or repair authority.
use super::super::store::{Artifact, Mode, Store};
use super::*;

const NAME: &str = "ordinary-installations.json";
const MAX_BYTES: usize = 64 * 1024;
pub(super) const RESERVATION: u64 = MAX_BYTES as u64 + 4096;

#[derive(Serialize)]
struct FileBinding<'a> {
    slot: &'a str,
    history: &'a str,
    file: &'a str,
    installation: &'a str,
}
#[derive(Serialize)]
struct Record<'a> {
    format: &'static str,
    candidate_hash: &'a str,
    choice_digest: &'a str,
    files: Vec<FileBinding<'a>>,
    disk_current: &'a BTreeMap<String, String>,
}

pub(super) struct Replicas {
    stores: Vec<Store>,
    retained: Vec<Option<Vec<u8>>>,
}
impl Replicas {
    pub(super) fn read(
        engine: &Engine,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<Self> {
        let mut stores = vec![Store::system(engine, materials, Mode::Read)?];
        for participant in &materials.record.disks {
            checkpoint(cancellation)?;
            stores.push(Store::disk(engine, materials, participant, Mode::Read)?);
        }
        let retained = stores
            .iter()
            .map(|store| store.read_optional_artifact(NAME, MAX_BYTES))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { stores, retained })
    }
    pub(super) fn has_original(&self) -> bool {
        self.retained.iter().any(Option::is_some)
    }
    pub(super) fn encode(
        frozen: &FrozenCandidate,
        bindings: &databases::Bindings,
    ) -> Result<Vec<u8>> {
        encode_metadata(
            &Record {
                format: "file-history-move-unpublished-installations",
                candidate_hash: frozen.hash(),
                choice_digest: frozen.choice_digest(),
                files: bindings
                    .files
                    .iter()
                    .map(|((slot, history, file), installation)| FileBinding {
                        slot,
                        history,
                        file,
                        installation,
                    })
                    .collect(),
                disk_current: &bindings.disks,
            },
            MAX_BYTES,
        )
    }
    pub(super) fn require_original(
        &self,
        bytes: &[u8],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        for (store, retained) in self.stores.iter().zip(&self.retained) {
            checkpoint(cancellation)?;
            if let Some(original) = retained {
                if original != bytes {
                    return Err(invalid(
                        "move pending database pairs differ from their frozen original installations",
                    ));
                }
                store.verify_artifacts(&[Artifact::Exact { name: NAME, bytes }], cancellation)?;
            }
        }
        Ok(())
    }
    pub(super) fn install(
        &self,
        bytes: &[u8],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        self.require_original(bytes, cancellation)?;
        for store in &self.stores {
            checkpoint(cancellation)?;
            store.install_artifacts(&[Artifact::Exact { name: NAME, bytes }], cancellation)?;
        }
        self.verify(bytes, cancellation)
    }
    pub(super) fn verify(
        &self,
        bytes: &[u8],
        cancellation: &HistoryWindowCancellation,
    ) -> Result<()> {
        for store in &self.stores {
            checkpoint(cancellation)?;
            store.verify_artifacts(&[Artifact::Exact { name: NAME, bytes }], cancellation)?;
        }
        Ok(())
    }
}
