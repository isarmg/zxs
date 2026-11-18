//! SYSTEM SQL rows and exact physical bytes from the same original business
//! input. No credentials, Candidate hash, additional vote or alternate root.
use super::{
    OrdinalBuilder, PreparedImage, Schema, ScopeClosureError as Failure,
    objects::{Domain, PreparedObject}, schema::{Material, Scope, SystemMaterial},
};
use crate::{
    Result, State, ValidatedConfig, business::NativeMoveCandidate,
    engine::index::preparation::SystemEncodingInput, history_package::AdmittedState, metadata_map::Reference,
};
use storage_contract::{Generation, PoolId, Sequence, working_tree::MaterialCommitment};

pub(in crate::engine) struct FrozenSystem {
    descriptor: PreparedObject,
    image: PreparedImage,
    material_bytes: usize,
}
impl FrozenSystem {
    pub(super) fn original(
        pool: PoolId, configuration: MaterialCommitment, empty: &Reference, original: &AdmittedState,
        checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let input = SystemEncodingInput::new(original, checkpoint)?;
        Self::measure(pool, configuration, empty, original.state(), &input, checkpoint)
    }
    pub(in crate::engine) fn move_candidate(
        config: &ValidatedConfig, candidate: &NativeMoveCandidate, checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let state = candidate.state();
        if candidate.result().sequence == 0 || state.sequence != candidate.result().sequence {
            return Err(Failure::PublicationMismatch.into());
        }
        let input = SystemEncodingInput::move_candidate(candidate, checkpoint)?;
        let configuration = MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
            "pool-config", &**config,
        )?);
        let empty = OrdinalBuilder::new(0).finish()?;
        Self::measure(config.pool_id.parse()?, configuration, empty.root(), state, &input, checkpoint)
    }
    fn measure(
        pool_id: PoolId, configuration_hash: MaterialCommitment, empty: &Reference, state: &State,
        input: &SystemEncodingInput<'_>, checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let bytes = input.encode(checkpoint)?;
        // Row-by-row field/schema/integrity validation is also the logical
        // root calculation. No second SQL encoding or historical alias.
        let logical_root = input.verified_scope_root(&bytes, checkpoint)?;
        let image = PreparedImage::new(Schema::System, bytes);
        let material = Material { scope: Scope::System, identity: pool_id, logical_root,
            representation: image.representation(), body_inventory: empty.clone() };
        let material_bytes = storage_contract::canonical_json(&material)?.len();
        let descriptor = PreparedObject::new(Domain::System, &SystemMaterial {
            pool_id, sequence: Sequence::new(state.sequence),
            business_generation: Generation::new(state.business_generation),
            topology_generation: Generation::new(state.topology_generation), configuration_hash, material,
        })?;
        Ok(Self { descriptor, image, material_bytes })
    }
    /// The existing native move staging slot retains these very same bytes.
    pub(in crate::engine) fn bytes(&self) -> &[u8] { self.image.as_bytes() }
    pub(in crate::engine) fn root(&self) -> MaterialCommitment { self.descriptor.hash() }
    pub(super) fn into_parts(self) -> (PreparedObject, PreparedImage, usize) {
        (self.descriptor, self.image, self.material_bytes)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/scope_system_material.rs"]
mod tests;
