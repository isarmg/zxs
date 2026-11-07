//! In-process delivery ordering for real, read-only candidate observations.
//! Request invalidation neither stops filesystem work nor proves discovery scope.
use super::{
    IndependentDisk, SourceCandidateBatch, SourceCandidateBatchError, SourceNode,
    observe_source_candidates,
};
use std::sync::Arc;

#[derive(Debug)]
struct RequestContext {
    node: SourceNode,
}

#[derive(Debug)]
enum State {
    NotRequested,
    Pending(Arc<RequestContext>),
    Observed(SourceCandidateBatch),
    Failed {
        context: Arc<RequestContext>,
        error: SourceCandidateBatchError,
    },
}

/// Borrow the current in-memory display state. An observed batch remains
/// historical metadata, with exactly the original incomplete/ambiguous status.
#[derive(Debug)]
pub enum SourceCandidateView<'a> {
    NotRequested,
    Pending(&'a SourceNode),
    Observed(&'a SourceCandidateBatch),
    Failed {
        node: &'a SourceNode,
        error: &'a SourceCandidateBatchError,
    },
}

/// Only the session owning this request may store its eventual response. This
/// non-cloneable request has no caller-supplied identity or wire constructor.
#[derive(Debug)]
pub struct SourceCandidateRequest {
    context: Arc<RequestContext>,
}

/// Opaque completion from the actual candidate verifier, not a caller-created
/// batch, error, discovered-disk DTO, or source-node comparison alone.
#[derive(Debug)]
pub struct SourceCandidateResponse {
    context: Arc<RequestContext>,
    result: Result<SourceCandidateBatch, SourceCandidateBatchError>,
}

impl SourceCandidateRequest {
    /// Run synchronous read-only verification once. Moving this request to a
    /// worker is permitted; scheduling, cancellation and backpressure belong to
    /// that caller. Invalidating a session does not abort an in-flight read.
    /// Consume explicitly selected safe directory handles. This producer reads
    /// the independent format itself, never trusts identity from a display DTO.
    /// It retains the same finite/incomplete semantics and opaque request owner.
    pub fn observe_held(
        self,
        candidates: Vec<platform_io::fs_safety::PrivateDirectory>,
    ) -> SourceCandidateResponse {
        let result = super::source_reader::observe_held(&self.context.node, candidates);
        SourceCandidateResponse {
            context: self.context,
            result,
        }
    }

    pub fn observe(self, candidates: &[&IndependentDisk]) -> SourceCandidateResponse {
        let result = observe_source_candidates(&self.context.node, candidates);
        SourceCandidateResponse {
            context: self.context,
            result,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceCandidateDelivery {
    /// The current response was stored. The view can be Observed or Failed.
    Updated,
    /// Superseded, invalidated, or from another owner; current state is unchanged.
    DiscardedStale,
}

/// One owner controls the visible result. Starting again, changing the source,
/// or invalidating clears the previous observation immediately. Old success
/// and old failure responses cannot replace a newer request or result.
#[derive(Debug)]
pub struct SourceCandidateSession {
    state: State,
}

impl Default for SourceCandidateSession {
    fn default() -> Self {
        Self {
            state: State::NotRequested,
        }
    }
}

impl SourceCandidateSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self, node: &SourceNode) -> SourceCandidateRequest {
        // Every request gets a distinct allocation. A stale request/response
        // retains its allocation, so address reuse cannot revive that request.
        let context = Arc::new(RequestContext { node: node.clone() });
        self.state = State::Pending(Arc::clone(&context));
        SourceCandidateRequest { context }
    }

    /// Clear display acceptance on close, cancellation, reselection, or loss of
    /// the owner's authorization. This does not cancel background filesystem I/O.
    pub fn invalidate(&mut self) {
        self.state = State::NotRequested;
    }

    pub fn view(&self) -> SourceCandidateView<'_> {
        match &self.state {
            State::NotRequested => SourceCandidateView::NotRequested,
            State::Pending(context) => SourceCandidateView::Pending(&context.node),
            State::Observed(batch) => SourceCandidateView::Observed(batch),
            State::Failed { context, error } => SourceCandidateView::Failed {
                node: &context.node,
                error,
            },
        }
    }

    /// Accept only this owner's still-pending request. The opaque response can
    /// only come from that request's real verifier call; comparing its node to
    /// itself is not the freshness check. No discovery completeness is inferred.
    pub fn receive(&mut self, response: SourceCandidateResponse) -> SourceCandidateDelivery {
        if !matches!(&self.state, State::Pending(context) if Arc::ptr_eq(context, &response.context))
        {
            return SourceCandidateDelivery::DiscardedStale;
        }
        let SourceCandidateResponse { context, result } = response;
        self.state = match result {
            Ok(batch) => State::Observed(batch),
            Err(error) => State::Failed { context, error },
        };
        SourceCandidateDelivery::Updated
    }
}
