//! Typed R2 dispatch adapter. It never starts a listener, parses raw JSON,
//! positions a host path, or invents success without a selected Store receipt.
use super::{Branch, Id, JsonMutation, MetadataMutation, Request};
mod project;
use crate::container_store as core;
use project::{Revoked, branch_key, describe, failed, receipt_for, save_node};
use std::io::{self, Read};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Access {
    List,
    Read(Id),
    Write(Id),
}
pub(crate) enum Decoded {
    Rename(String),
    ReplaceTags(Vec<String>),
    CreateBranchFrom(Id),
}
pub(crate) enum Body<R> {
    Empty,
    Blob(R),
    /// Exactly one bounded validated JSON object; length is bytes consumed.
    Json {
        length: u64,
        decoded: Decoded,
    },
}
pub(crate) enum Reply {
    Containers(Vec<core::Container>),
    Container(core::Container),
    Tags(Vec<String>),
    Node(core::NodeBytes),
    BranchNodes(Vec<(core::NodeId, core::Node)>),
    Mutation(core::Receipt),
    Recycle {
        selected: core::RecycleReceipt,
        source_cleanup_pending: bool,
    },
    Receipt(ReceiptLookup),
}
pub(crate) enum ReceiptLookup {
    Mutation(core::Receipt),
    /// Durable R selection only, not proof that cleanup finished.
    Recycle(core::RecycleReceipt),
    NotFound,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Denied,
    BadBody,
    Missing,
    Conflict,
    InvalidBody,
    Unavailable,
}
fn access(request: &Request) -> Access {
    match request {
        Request::List => Access::List,
        Request::Container(id) | Request::Tags(id) => Access::Read(*id),
        Request::NodeData { container, .. }
        | Request::BranchNodes { container, .. }
        | Request::Receipt { container, .. } => Access::Read(*container),
        Request::SaveNode(save) => Access::Write(save.container),
        Request::CreateBranch { container, .. } => Access::Write(*container),
        Request::Delete(scope) | Request::Json { scope, .. } => Access::Write(scope.container),
    }
}
pub(super) fn mutation(container: Id, request: Id) -> core::Mutation {
    core::Mutation {
        container: container.bytes(),
        request: request.bytes(),
    }
}
fn metadata(scope: MetadataMutation) -> core::Mutation {
    mutation(scope.container, scope.operation)
}

/// Callers must supply their actual authenticated liveness/authority checks.
/// This callback is rechecked inside storage selection and must not itself
/// lock the Store. A trusted compile-only adapter must never stand in for it.
pub(crate) fn dispatch<R: Read>(
    store: &core::Store,
    request: Request,
    body: Body<R>,
    allowed: impl Fn(Access) -> bool,
) -> Result<Reply, Failure> {
    let requested = access(&request);
    if !allowed(requested) {
        return Err(Failure::Denied);
    }
    let authorize = || {
        if allowed(requested) {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, Revoked))
        }
    };
    match (request, body) {
        (Request::List, Body::Empty) => Ok(Reply::Containers(
            store
                .list()
                .map_err(|_| Failure::Unavailable)?
                .into_iter()
                .filter(|c| allowed(Access::Read(Id(c.id))))
                .collect(),
        )),
        (Request::Container(id), Body::Empty) => Ok(Reply::Container(describe(store, id)?)),
        (Request::Tags(id), Body::Empty) => Ok(Reply::Tags(describe(store, id)?.tags)),
        (Request::NodeData { container, node }, Body::Empty) => Ok(Reply::Node(
            store
                .node(container.bytes(), node.bytes())
                .map_err(|_| Failure::Unavailable)?
                .ok_or(Failure::Missing)?,
        )),
        (
            Request::Receipt {
                container,
                operation,
            },
            Body::Empty,
        ) => Ok(Reply::Receipt(lookup_receipt(store, container, operation))),
        (Request::SaveNode(save), Body::Blob(reader)) => save_node(store, save, reader, authorize),
        (Request::Delete(scope), Body::Empty) => recycle(store, scope, authorize),
        (
            Request::Json {
                scope,
                mutation,
                length,
            },
            Body::Json {
                length: actual,
                decoded,
            },
        ) if length == actual => {
            let change = match (mutation, decoded) {
                (JsonMutation::Rename, Decoded::Rename(name)) => core::Change::Rename {
                    expected_sequence: scope.revision,
                    name,
                },
                (JsonMutation::ReplaceTags, Decoded::ReplaceTags(tags)) => core::Change::Tags {
                    expected_sequence: scope.revision,
                    tags,
                },
                _ => return Err(Failure::BadBody),
            };
            let op = metadata(scope);
            let receipt = store
                .change(op, change.clone(), authorize)
                .map_err(|error| failed(&error))?;
            receipt_for(receipt, op, &change)
        }
        (
            Request::CreateBranch {
                container,
                operation,
                branch,
                length,
            },
            Body::Json {
                length: actual,
                decoded: Decoded::CreateBranchFrom(from),
            },
        ) if length == actual => {
            let op = mutation(container, operation);
            let change = core::Change::Branch {
                name: branch_key(Branch::Named(branch)),
                from: from.bytes(),
            };
            let receipt = store
                .change(op, change.clone(), authorize)
                .map_err(|error| failed(&error))?;
            receipt_for(receipt, op, &change)
        }
        (Request::BranchNodes { container, branch }, Body::Empty) => Ok(Reply::BranchNodes(
            store
                .branch_nodes(container.bytes(), &branch_key(branch))
                .map_err(|_| Failure::Unavailable)?
                .ok_or(Failure::Missing)?,
        )),
        _ => Err(Failure::BadBody),
    }
}

fn lookup_receipt(store: &core::Store, container: Id, operation: Id) -> ReceiptLookup {
    let op = mutation(container, operation);
    match (store.receipt(op), store.recycle_receipt(op)) {
        (Ok(Some(r)), Ok(None)) if r.mutation == op => ReceiptLookup::Mutation(r),
        (Ok(None), Ok(Some(r))) if r.mutation == op => ReceiptLookup::Recycle(r),
        (Ok(None), Ok(None)) => ReceiptLookup::NotFound,
        _ => ReceiptLookup::Unknown,
    }
}

fn recycle(
    store: &core::Store,
    scope: MetadataMutation,
    authorize: impl Fn() -> io::Result<()>,
) -> Result<Reply, Failure> {
    let op = metadata(scope);
    let (selected, source_cleanup_pending) =
        match store.move_to_recycle(op, scope.revision, authorize) {
            Ok(receipt) => (receipt, false),
            Err(core::Error::CleanupPending { receipt, .. }) => (receipt, true),
            Err(error) => return Err(failed(&error)),
        };
    if selected.mutation != op || selected.original_sequence != scope.revision {
        return Err(Failure::Unavailable);
    }
    Ok(Reply::Recycle {
        selected,
        source_cleanup_pending,
    })
}
