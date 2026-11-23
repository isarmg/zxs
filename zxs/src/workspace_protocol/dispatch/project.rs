use super::super::{SaveNode, Target};
use super::{Branch, Failure, Reply, mutation};
use crate::container_store as core;
use std::io::{self, Read};

pub(super) fn branch_key(branch: Branch) -> String {
    match branch {
        Branch::Main => String::from(core::MAIN),
        Branch::Named(id) => {
            let mut out = String::with_capacity(32);
            for byte in id.bytes() {
                for n in [byte >> 4, byte & 15] {
                    out.push(char::from(b"0123456789abcdef"[usize::from(n)]));
                }
            }
            out
        }
    }
}

pub(super) fn receipt_for(
    receipt: core::Receipt,
    op: core::Mutation,
    change: &core::Change,
) -> Result<Reply, Failure> {
    if receipt.mutation != op || receipt.change != *change || receipt.sequence == 0 {
        return Err(Failure::Unavailable);
    }
    Ok(Reply::Mutation(receipt))
}

pub(super) fn failed(error: &core::Error) -> Failure {
    match error {
        core::Error::WrongLength | core::Error::WrongDigest => Failure::InvalidBody,
        core::Error::Conflict(
            core::Conflict::MissingContainer
            | core::Conflict::MissingBranch
            | core::Conflict::MissingNode,
        ) => Failure::Missing,
        core::Error::Conflict(core::Conflict::Invalid(error)) => {
            if error.kind() == io::ErrorKind::InvalidInput {
                Failure::BadBody
            } else {
                Failure::Unavailable
            }
        }
        core::Error::Conflict(_) => Failure::Conflict,
        core::Error::Io(error)
            if error
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Revoked>) =>
        {
            Failure::Denied
        }
        core::Error::Io(_) | core::Error::Uncertain(_) | core::Error::CleanupPending { .. } => {
            Failure::Unavailable
        }
    }
}

pub(super) fn save_node<R: Read>(
    store: &core::Store,
    save: SaveNode,
    reader: R,
    authorize: impl Fn() -> io::Result<()>,
) -> Result<Reply, Failure> {
    let op = mutation(save.container, save.operation);
    let node = save.node.bytes();
    let body = core::Body {
        length: save.length,
        sha256: save.sha256,
    };
    let raw = save.body(reader);
    let target = match save.target {
        Target::FirstMain { name } => core::Target::FirstMain { name },
        Target::Branch {
            branch,
            expected_head,
        } => core::Target::Branch {
            name: branch_key(branch),
            expected_head: expected_head.bytes(),
        },
        Target::HistoricalMain {
            new_branch,
            original,
        } => core::Target::HistoricalMain {
            new_branch: branch_key(Branch::Named(new_branch)),
            original: original.bytes(),
        },
    };
    let exact = core::Change::Save { target, node, body };
    let receipt = store
        .save(op, exact.clone(), raw, authorize)
        .map_err(|error| failed(&error))?;
    receipt_for(receipt, op, &exact)
}

#[derive(Debug)]
pub(super) struct Revoked;
impl std::fmt::Display for Revoked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("workspace permission was revoked")
    }
}
impl std::error::Error for Revoked {}
