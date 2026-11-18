//! The only encoder for the proposed SYSTEM record. Reading compares with
//! fresh bytes from the original held resources; saved JSON is never promoted
//! into a request, a proof of authority, a fence, or writer permission.
use super::*;
use crate::file_stamp::FileStamp;
use crate::foreign_import::{ForeignAdmissionRecipe, PreservedMaterial};
use std::path::PathBuf;
use storage_contract::Principal;

pub(super) const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Serialize)]
enum Format {
    #[serialization(rename = "foreign-preserved-chain-intent")]
    PreservedChain,
}

macro_rules! commitment {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Serialize)]
        #[serialization(transparent)]
        struct $name(ForeignEvidenceDigest);
    };
}
commitment!(Request);
commitment!(Chain);
commitment!(RegistrationProposal);
commitment!(CurrentConfig);
commitment!(CurrentSnapshot);
commitment!(CurrentState);
commitment!(TargetConfig);
commitment!(Recipe);
commitment!(Intent);

pub(super) struct RequestDigests {
    request: Request,
    chain: Chain,
    proposal: RegistrationProposal,
}
impl RequestDigests {
    pub(super) fn original(request: &ForeignIdentityTargetRequest) -> Self {
        Self {
            request: Request(request.request_digest),
            chain: Chain(request.chain_digest),
            proposal: RegistrationProposal(request.registration_proposal_digest),
        }
    }
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Root {
    level: u32,
    registration: DiskRegistrationIdentity,
    upstream_slot_id: Option<name_policy::SlotId>,
    metadata_member: bool,
    path: PathBuf,
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
    material: PreservedMaterial,
}

impl Root {
    pub(super) fn observed(
        config: &Config,
        disk: &DiskConfig,
        physical: FileIdentity,
        material: PreservedMaterial,
    ) -> Result<Self> {
        let registration = DiskRegistrationIdentity {
            pool_id: config.pool_id.clone(),
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        };
        Ok(Self {
            level: disk.level,
            registration,
            upstream_slot_id: disk.upstream_slot_id.as_deref()
                .map(name_policy::SlotId::parse).transpose()?,
            metadata_member: disk.metadata_member,
            path: disk.root.clone(),
            device: physical.device,
            inode: physical.inode,
            material,
        })
    }
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Record<'a> {
    format: Format,
    operation_id: OperationId,
    principal: Principal,
    request_digest: Request,
    chain_digest: Chain,
    registration_proposal_digest: RegistrationProposal,
    current_configuration_hash: CurrentConfig,
    #[serialization(with = "storage_contract::u64_string")]
    current_sequence: u64,
    current_snapshot_hash: Option<CurrentSnapshot>,
    current_state_hash: CurrentState,
    target_configuration_hash: TargetConfig,
    recipe_hash: Recipe,
    original: &'a ForeignOriginalStateReport,
    roots: Vec<Root>,
}

/// A past observation has no authority to write. Rebuild the complete proof
/// while its original owners are held when comparing a saved record.
pub(in crate::engine::membership_transition) struct Observed {
    operation: OperationId,
    bytes: Vec<u8>,
    digest: Intent,
}
impl Observed {
    pub(super) fn freeze(
        principal: Principal,
        request: RequestDigests,
        snapshot: Option<ForeignEvidenceDigest>,
        recipe: &ForeignAdmissionRecipe,
        roots: Vec<Root>,
    ) -> Result<Self> {
        let identities = recipe.identities();
        let baseline = &identities.baseline;
        let record = Record {
            format: Format::PreservedChain,
            operation_id: baseline.operation_id,
            principal,
            request_digest: request.request,
            chain_digest: request.chain,
            registration_proposal_digest: request.proposal,
            current_configuration_hash: CurrentConfig(parse(&baseline.configuration_hash)?),
            current_sequence: baseline.sequence,
            current_snapshot_hash: snapshot.map(CurrentSnapshot),
            current_state_hash: CurrentState(parse(&baseline.state_hash)?),
            target_configuration_hash: TargetConfig(parse(&baseline.proposed_configuration_hash)?),
            recipe_hash: Recipe(parse(&object_hash("foreign-preserved-recipe", recipe)?)?),
            original: &identities.original,
            roots,
        };
        let (bytes, digest) = encode(&record)?;
        Ok(Self { operation: baseline.operation_id, bytes, digest })
    }

    pub(super) fn require_original_if_present(&self, system: &DiskRoot) -> Result<()> {
        let directory = system.path().join(layout::FOREIGN_ENROLLMENT.as_path());
        let Some(metadata) = system.optional_relative_metadata(&layout::FOREIGN_ENROLLMENT.as_relative())? else {
            return Ok(());
        };
        let identity = FileIdentity::from_metadata(&metadata);
        let parent = system.private_child_matching(&directory, identity)?;
        let leaf = layout::foreign_enrollment(self.operation);
        if let Some(file) = parent.open_optional_read_entry(&leaf)? {
            let stamp = FileStamp::from_metadata(&file.metadata()?);
            let actual = read_file_bytes(file, MAX_RECORD_BYTES)?;
            // Exact canonical byte equality also forbids duplicate, unknown or
            // omitted fields, alternate numeric forms and a missing vs null value.
            if self.bytes != actual
                || stamp != FileStamp::from_metadata(&parent.open_read_entry(&leaf)?.metadata()?)
            {
                return Err(Invalid::Original.into());
            }
        }
        if system.directory_identity(&directory)? != identity {
            return Err(Invalid::Binding.into());
        }
        Ok(())
    }
}

fn parse(text: &str) -> Result<ForeignEvidenceDigest> {
    text.parse().map_err(|source| Error::from(std::io::Error::other(source)))
}

fn encode(value: &impl Serialize) -> Result<(Vec<u8>, Intent)> {
    crate::foreign_import::bounded::encoded_length(value, MAX_RECORD_BYTES, || {
        Invalid::RecordBudget.into()
    })?;
    let (bytes, digest) = storage_contract::encode_hashed_object(
        "foreign-preserved-chain-intent", value,
    )?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Invalid::RecordBudget.into());
    }
    Ok((bytes, Intent(parse(&digest)?)))
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/foreign_import/preserved_record_tests.rs"]
mod tests;
