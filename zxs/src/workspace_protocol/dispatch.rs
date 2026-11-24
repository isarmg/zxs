//! Typed R2 dispatch adapter. It never starts a listener, parses raw JSON,
//! positions a host path, or invents success without a selected Store receipt.
use super::{Branch, Id, JsonMutation, MetadataMutation, Request};
mod project;
use crate::container_store as core;
use project::{Revoked, branch_key, failed, receipt_for, save_node};
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
    Catalog(core::CatalogPage),
    Detail(core::DetailPage),
    Tags(Vec<String>),
    Node(core::NodeBytes),
    BranchHistory(core::HistoryPage),
    Mutation(core::Receipt),
    Recycle {
        selected: core::RecycleReceipt,
        source_cleanup_pending: bool,
    },
    Receipt(Option<core::OperationReceipt>),
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
        Request::ListCompact { .. } => Access::List,
        Request::Tags(id) => Access::Read(*id),
        Request::NodeData { container, .. }
        | Request::ContainerDetail { container, .. }
        | Request::BranchHistory { container, .. }
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

fn list_catalog(
    store: &core::Store,
    after: Option<Id>,
    allowed: &impl Fn(Access) -> bool,
) -> Result<Reply, Failure> {
    let readable = |id| allowed(Access::Read(Id(id)));
    let page = store
        .catalog_page(after.map(Id::bytes), readable)
        .map_err(|_| Failure::Unavailable)?;
    if !allowed(Access::List) || page.items.iter().any(|row| !readable(row.id)) {
        return Err(Failure::Denied);
    }
    Ok(Reply::Catalog(page))
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
    let result = execute(store, request, body, &allowed, authorize);
    // Never expose a read projection (including a negative receipt) after
    // revocation. Writers recheck at the exact selection/publication cut.
    if matches!(requested, Access::Read(_)) && !allowed(requested) {
        return Err(Failure::Denied);
    }
    result
}

fn execute<R: Read>(
    store: &core::Store,
    request: Request,
    body: Body<R>,
    allowed: &impl Fn(Access) -> bool,
    authorize: impl Fn() -> io::Result<()>,
) -> Result<Reply, Failure> {
    match (request, body) {
        (Request::ListCompact { after }, Body::Empty) => list_catalog(store, after, allowed),
        (
            Request::ContainerDetail {
                container,
                sequence,
                after,
            },
            Body::Empty,
        ) => read_detail(store, container, sequence, after),
        (Request::Tags(id), Body::Empty) => Ok(Reply::Tags(
            store
                .tags(id.bytes())
                .map_err(|_| Failure::Unavailable)?
                .ok_or(Failure::Missing)?,
        )),
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
        ) => store
            .operation_receipt(mutation(container, operation))
            .map(Reply::Receipt)
            .map_err(|_| Failure::Unavailable),
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
        (
            Request::BranchHistory {
                container,
                branch,
                head,
                next,
            },
            Body::Empty,
        ) => read_history(store, container, branch, head, next),
        _ => Err(Failure::BadBody),
    }
}

fn read_detail(
    store: &core::Store,
    container: Id,
    sequence: Option<u64>,
    after: Option<Branch>,
) -> Result<Reply, Failure> {
    let after = after.map(branch_key);
    let page = store
        .detail_page(container.bytes(), sequence, after.as_deref())
        .map_err(|error| failed(&error))?
        .ok_or(Failure::Missing)?;
    Ok(Reply::Detail(page))
}

fn read_history(
    store: &core::Store,
    container: Id,
    branch: Branch,
    head: Id,
    next: Option<Id>,
) -> Result<Reply, Failure> {
    let page = store
        .branch_history(
            container.bytes(),
            &branch_key(branch),
            head.bytes(),
            next.map(Id::bytes),
        )
        .map_err(|error| failed(&error))?
        .ok_or(Failure::Missing)?;
    Ok(Reply::BranchHistory(page))
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
            Err(core::Error::CleanupPending { receipt, .. }) => (*receipt, true),
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
