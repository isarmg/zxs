//! Typed, source-scoped history graph drafts. New object names do not confer a
//! target publication, target placement, authentication or a live State.
use super::*;
use crate::history_package::HistoryManifest;

mod manifest;
mod records;
pub(super) mod remap;

const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignHistoryGraphReport {
    pub identities: ForeignIdentityClosureReport,
    pub graph_digest: String,
    pub histories: usize,
    pub versions: usize,
    pub unavailable_versions: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub graph_bytes: u64,
}

pub(in crate::foreign_import) struct PreparedGraph {
    pub(in crate::foreign_import) report: ForeignHistoryGraphReport,
    pub(in crate::foreign_import) bytes: Vec<u8>,
}

#[derive(Serialize)]
#[serialization(rename_all = "snake_case")]
enum SourceScope {
    OriginalPoolOnly,
}
/// Every preserved domain is explicit; these records are deliberately not a
/// HistoryPackage/State archive and must not enter their restore interfaces.
#[derive(Serialize)]
struct PreservedDomains {
    publication_sequences_operations_and_actors: SourceScope,
    placement_slots_generations_and_smb_binding: SourceScope,
    paths_storage_names_and_backup_prefixes: SourceScope,
    provenance_and_external_boundary_references: SourceScope,
}
#[derive(Serialize)]
#[serialization(rename_all = "snake_case")]
enum Availability {
    VerifiedLocal,
    TemporarilyUnavailable,
    RecordedPurged,
    RecordedMigrated,
}
impl From<ForeignVersionAvailability> for Availability {
    fn from(value: ForeignVersionAvailability) -> Self {
        match value {
            ForeignVersionAvailability::VerifiedLocal => Self::VerifiedLocal,
            ForeignVersionAvailability::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ForeignVersionAvailability::RecordedPurged => Self::RecordedPurged,
            ForeignVersionAvailability::RecordedMigrated => Self::RecordedMigrated,
        }
    }
}
#[derive(Serialize)]
struct History<'a> {
    original_history_id: &'a str,
    original_manifest_hash: &'a str,
    original_primary_group: &'a str,
    original_version_availability: BTreeMap<&'a str, Availability>,
    original_version_publications: &'a BTreeMap<String, String>,
    graph: HistoryManifest,
}
#[derive(Serialize)]
struct Archive<'a> {
    format: &'static str,
    identities: &'a ForeignIdentityClosureReport,
    preserved_domains: PreservedDomains,
    histories: Vec<History<'a>>,
}
impl PreparedGraph {
    pub(in crate::foreign_import) fn capture(
        target: &ForeignIdentityTarget,
        source: &ForeignImportPreview,
    ) -> Result<Self> {
        let identities = Prepared::capture(target, source)?;
        let mapper = remap::Remap::new(&identities.objects);
        let mut histories = Vec::new();
        let mut bytes_left = MAX_GRAPH_BYTES;
        let mut versions = 0;
        let mut unavailable_versions = 0;
        for original in source.histories() {
            // Charge before cloning the source graph. Stable ID substitution
            // preserves string lengths; the final envelope has a separate cap.
            let length = encoded_length(original.manifest(), MAX_HISTORY_BYTES)?;
            bytes_left = bytes_left.checked_sub(length).ok_or_else(budget)?;
            let graph = mapper.manifest(original.manifest())?;
            graph.validate()?;
            versions += graph.versions.len();
            unavailable_versions += original
                .availability()
                .values()
                .filter(|value| matches!(value, ForeignVersionAvailability::TemporarilyUnavailable))
                .count();
            histories.push(History {
                original_history_id: &original.manifest().history.history_tree_id,
                original_manifest_hash: original.manifest_hash(),
                original_primary_group: original.group_id(),
                original_version_availability: original
                    .availability()
                    .iter()
                    .map(|(id, state)| (id.as_str(), Availability::from(*state)))
                    .collect(),
                original_version_publications: original.origin_publications(),
                graph,
            });
        }
        let archive = Archive {
            format: "zxs-foreign-source-scoped-mapped-history-graphs",
            identities: &identities.report,
            preserved_domains: PreservedDomains {
                publication_sequences_operations_and_actors: SourceScope::OriginalPoolOnly,
                placement_slots_generations_and_smb_binding: SourceScope::OriginalPoolOnly,
                paths_storage_names_and_backup_prefixes: SourceScope::OriginalPoolOnly,
                provenance_and_external_boundary_references: SourceScope::OriginalPoolOnly,
            },
            histories,
        };
        encoded_length(&archive, MAX_GRAPH_BYTES)?;
        let (bytes, graph_digest) = storage_contract::encode_hashed_object(
            "foreign-source-scoped-mapped-history-graphs",
            &archive,
        )?;
        let report = ForeignHistoryGraphReport {
            identities: identities.report.clone(),
            graph_digest,
            histories: archive.histories.len(),
            versions,
            unavailable_versions,
            graph_bytes: bytes.len() as u64,
        };
        source.revalidate_source()?;
        Ok(Self { report, bytes })
    }
}
/// Compact and canonical encodings have equal byte lengths. Count before any
/// output allocation, without constructing a Value tree for the preflight.
pub(super) fn encoded_length<T: Serialize + ?Sized>(value: &T, maximum: usize) -> Result<usize> {
    struct Limit {
        remaining: usize,
        exceeded: bool,
    }
    impl std::io::Write for Limit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                self.exceeded = true;
                return Err(std::io::Error::other(
                    "foreign mapped graph byte budget exceeded",
                ));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut limit = Limit {
        remaining: maximum,
        exceeded: false,
    };
    match json_codec::to_writer(&mut limit, value) {
        Ok(()) => Ok(maximum - limit.remaining),
        Err(_) if limit.exceeded => Err(budget()),
        Err(error) => Err(error.into()),
    }
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "foreign mapped graphs exceed 8 MiB per history or 64 MiB per archive",
    )
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/foreign_import/history_graph_tests.rs"]
mod tests;
