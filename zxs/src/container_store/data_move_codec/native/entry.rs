//! Authorize exactly one explicit request. Neither paths nor DATA choices
//! are taken from the sealed handoff.
use super::{
    ACCESS, ACTIVE_PREFIX, Digest, Lease, MAX_PRIVATE_ENTRIES, MOVE_PARENT, Roots, Scope, Selected,
    invalid, native_name, operand,
    records::{self, Completion, HistoryRequest, history},
};
use crate::{
    container_store::{DataRoot, Id, hex},
    fs::HeldDir,
};
use std::{
    ffi::{CStr, CString},
    io,
};

const SERVER_STAGE: &[u8] = b".server-stage-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MoveRequest {
    pub(crate) container: Id,
    pub(crate) operation: Id,
    operands: (Digest, Digest),
}

impl MoveRequest {
    pub(crate) fn checked(
        container: Id,
        operation: Id,
        operands: (Digest, Digest),
    ) -> io::Result<Self> {
        if container == [0; 16]
            || operation == [0; 16]
            || operands.0 == [0; 32]
            || operands.1 == [0; 32]
            || operands.0 == operands.1
        {
            return Err(invalid("invalid exact offline DATA move request"));
        }
        Ok(Self {
            container,
            operation,
            operands,
        })
    }

    fn operation_name(self) -> io::Result<CString> {
        native_name(&format!("{}-{}", hex(self.container), hex(self.operation)))
    }

    fn active_name(self) -> io::Result<CString> {
        native_name(&format!(
            "{ACTIVE_PREFIX}{}-{}",
            hex(self.container),
            hex(self.operation)
        ))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MoveRoles<'a> {
    pub(crate) state: &'a HeldDir,
    pub(crate) system: &'a HeldDir,
    pub(crate) recycle: &'a HeldDir,
    pub(crate) data: &'a [DataRoot],
    pub(crate) pool: Id,
    pub(crate) system_token: Digest,
}

fn require_idle(state: &HeldDir, prefix: &[u8], allowed: Option<&CStr>) -> io::Result<()> {
    let mut remaining = MAX_PRIVATE_ENTRIES;
    state.each_name(ACCESS, |name| {
        remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| invalid("the original STATE inventory exceeds its bound"))?;
        if name.starts_with(prefix) && allowed.is_none_or(|same| same.to_bytes() != name) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "a pending offline operation guards this pool",
            ));
        }
        Ok(())
    })
}

pub(crate) fn require_move_absent(state: &HeldDir) -> io::Result<()> {
    require_idle(state, ACTIVE_PREFIX.as_bytes(), None)
}

pub(crate) fn require_move_scope(state: &HeldDir, request: MoveRequest) -> io::Result<()> {
    let active = request.active_name()?;
    require_idle(state, SERVER_STAGE, Some(&active))
}

pub(crate) fn require_stage_absent(state: &HeldDir) -> io::Result<()> {
    require_idle(state, SERVER_STAGE, None)
}

fn held_operand(data: &[DataRoot], expected: Digest) -> io::Result<&DataRoot> {
    let mut found = None;
    for disk in data {
        if operand(&disk.root)? == expected && found.replace(disk).is_some() {
            return Err(invalid(
                "the DATA move operand does not uniquely select a held role",
            ));
        }
    }
    found.ok_or_else(|| invalid("the original DATA move operand is not enrolled"))
}

fn original_journal(system: &HeldDir, request: MoveRequest) -> io::Result<Option<HeldDir>> {
    let Some(parent) = system.child_directory(MOVE_PARENT)? else {
        return Ok(None);
    };
    parent.child_directory(&request.operation_name()?)
}

pub(crate) fn execute_move(
    roles: MoveRoles<'_>,
    request: MoveRequest,
    outer: &dyn Fn() -> io::Result<()>,
    pulse: &dyn Fn() -> io::Result<()>,
) -> io::Result<Completion> {
    outer()?;
    require_move_scope(roles.state, request)?;
    let identity = (
        Scope {
            pool: roles.pool,
            container: request.container,
            operation: request.operation,
        },
        roles.system_token,
        request.operands,
    );
    let journal = original_journal(roles.system, request)?;
    // Exact DONE is first. An incorrect or unavailable named DONE is not
    // absence and it must not fall back to old DATA or to another operation.
    if let Some(at) = &journal
        && at.pin_record(c"DONE")?.is_some()
    {
        let owner = Lease::hold(roles.system, c"container-owner")?;
        return history(HistoryRequest {
            state: roles.state,
            system: roles.system,
            system_owner: &owner.original,
            identity,
            live: outer,
        });
    }
    let source = held_operand(roles.data, request.operands.0)?;
    let target = held_operand(roles.data, request.operands.1)?;
    let roots = Roots {
        state: roles.state,
        system: roles.system,
        recycle: roles.recycle,
        data: (source, target),
    };
    let selected = Selected::retain_live(roots, identity, outer, pulse)?;
    records::advance(&selected, journal.as_ref())
}

#[cfg(test)]
#[path = "../../../../tests/unit/data_move_worker_gate.rs"]
mod tests;
