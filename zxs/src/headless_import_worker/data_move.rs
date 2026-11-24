//! Exact kind-50 local handoff: the sealed request provides comparison
//! identities, not openable paths or data descriptors.
use super::{Channel, Payload, Sha256, fixed, inherited_order_with_limit, refused, role_binding};
use crate::{
    container_store::{MoveCompletion, MoveRequest, require_stage_absent},
    headless_offline,
};
use std::io;

const BYTES: usize = 168;
const KIND: u8 = 50;
const ROLE_DOMAIN: &[u8] = b"ZXS disk move live roles v1\0";

struct Order {
    binding: [u8; 32],
    scope: [u8; 32],
    request: MoveRequest,
}

impl Order {
    fn decode(mut raw: &[u8]) -> io::Result<Self> {
        if raw.len() != BYTES || fixed::<8>(&mut raw)? != *b"ZXDMV1\0\0" {
            return Err(refused());
        }
        let binding = fixed(&mut raw)?;
        let scope = fixed(&mut raw)?;
        let container = fixed(&mut raw)?;
        let operation = fixed(&mut raw)?;
        let operands = (fixed(&mut raw)?, fixed(&mut raw)?);
        let request = MoveRequest::checked(container, operation, operands)?;
        if scope == [0; 32] || !raw.is_empty() {
            return Err(refused());
        }
        Ok(Self {
            binding,
            scope,
            request,
        })
    }
}

pub(crate) fn run() -> io::Result<()> {
    let (socket, parent, raw) = inherited_order_with_limit(BYTES)?;
    let order = Order::decode(&raw)?;
    let (fence, selected) = headless_offline::retain_for_move(order.request)?;
    let require_state = |state: &crate::fs::HeldDir| fence.require(state);
    let roles = selected.verify_live_roles(require_state)?;
    if role_binding(ROLE_DOMAIN, fence.binding, &roles)? != order.binding {
        return Err(refused());
    }
    let mut hash = Sha256::new();
    hash.update(&raw)?;
    let channel = Channel {
        socket,
        parent,
        scope: order.scope,
        digest: hash.finish(),
        kind: KIND,
    };
    let pulse = || channel.require_parent();
    let result = selected.offline_move(order.request, &roles, require_state, &pulse)?;
    pulse()?;
    if selected.verify_live_roles(|state| {
        require_state(state)?;
        require_stage_absent(state)
    })? != roles
    {
        return Err(refused());
    }
    pulse()?;
    channel.send(terminal(order.request, result))
}

fn terminal(request: MoveRequest, completed: MoveCompletion) -> Payload {
    let (status, done) = match completed {
        MoveCompletion::Completed(id) => (1, id),
        MoveCompletion::Historical(id) => (2, id),
    };
    let mut out = [0; 65];
    out[0] = status;
    out[1..17].copy_from_slice(&request.container);
    out[17..33].copy_from_slice(&request.operation);
    out[33..65].copy_from_slice(&done);
    out
}

#[cfg(test)]
#[path = "../../tests/unit/offline_data_move_worker.rs"]
mod tests;
