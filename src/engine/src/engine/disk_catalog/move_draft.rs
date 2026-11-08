//! Only the private native move draft can reach this unaccepted disk encoder.
//! Bytes stay under the move's immutable staging manifest, never the root alias.
use super::*;

pub(in crate::engine) fn prepare_move_current(
    native: &crate::business::NativeMoveDraft,
    identity: DiskRegistrationIdentity,
) -> Result<Vec<u8>> {
    let proposed = snapshot(native.proposed_state(), identity)?;
    let hash = storage_contract::object_hash("disk-current-catalog", &proposed)?;
    let bytes = database::encode(&proposed, &hash)?;
    if database::read(&bytes, &proposed.identity)? != proposed {
        return Err(invalid("unaccepted move disk database changed its exact proposed scope"));
    }
    Ok(bytes)
}
