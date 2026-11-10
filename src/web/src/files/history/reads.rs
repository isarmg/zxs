//! A fixed number of owned reads; every relation kind shares one slot.
use crate::api::ReadCancellation;
use crate::files::review::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReviewOwner {
    pub(crate) kind: Kind,
    pub(crate) instance: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resource {
    Branches,
    Merges,
    Relation,
    Field,
    Review,
}

#[derive(Clone, Default)]
struct Slot {
    generation: u64,
    cancellation: Option<ReadCancellation>,
    loading: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Reads {
    branches: Slot,
    merges: Slot,
    relation: Slot,
    field: Slot,
    review: Slot,
    review_owner: Option<ReviewOwner>,
}

impl Reads {
    fn slot(&self, resource: Resource) -> &Slot {
        match resource {
            Resource::Branches => &self.branches,
            Resource::Merges => &self.merges,
            Resource::Relation => &self.relation,
            Resource::Field => &self.field,
            Resource::Review => &self.review,
        }
    }

    fn slot_mut(&mut self, resource: Resource) -> &mut Slot {
        match resource {
            Resource::Branches => &mut self.branches,
            Resource::Merges => &mut self.merges,
            Resource::Relation => &mut self.relation,
            Resource::Field => &mut self.field,
            Resource::Review => &mut self.review,
        }
    }

    pub(crate) fn begin(
        &mut self,
        resource: Resource,
        cancellation: Option<ReadCancellation>,
    ) -> u64 {
        self.cancel(resource);
        let slot = self.slot_mut(resource);
        slot.cancellation = cancellation;
        slot.loading = true;
        slot.generation
    }

    pub(crate) fn current(&self, resource: Resource, generation: u64) -> bool {
        let slot = self.slot(resource);
        slot.loading && slot.generation == generation
    }

    /// A sequential review gets a fresh controller for each fetch: the API
    /// request guard aborts its controller when that one fetch is released.
    pub(crate) fn replace_owner(
        &mut self,
        resource: Resource,
        generation: u64,
        cancellation: ReadCancellation,
    ) -> bool {
        if !self.current(resource, generation) {
            return false;
        }
        let slot = self.slot_mut(resource);
        if let Some(previous) = slot.cancellation.replace(cancellation) {
            previous.abort();
        }
        true
    }

    pub(crate) fn complete(&mut self, resource: Resource, generation: u64) -> bool {
        if !self.current(resource, generation) {
            return false;
        }
        if resource == Resource::Review {
            self.review_owner = None;
        }
        let slot = self.slot_mut(resource);
        slot.loading = false;
        slot.cancellation = None;
        true
    }

    pub(crate) fn cancel(&mut self, resource: Resource) {
        if resource == Resource::Review {
            self.review_owner = None;
        }
        let slot = self.slot_mut(resource);
        if let Some(cancellation) = slot.cancellation.take() {
            cancellation.abort();
        }
        slot.generation += 1;
        slot.loading = false;
    }

    pub(crate) fn begin_review(&mut self, owner: ReviewOwner) -> u64 {
        let generation = self.begin(Resource::Review, None);
        self.review_owner = Some(owner);
        generation
    }

    pub(crate) fn current_review(&self, owner: ReviewOwner, generation: u64) -> bool {
        self.review_owner == Some(owner) && self.current(Resource::Review, generation)
    }

    pub(crate) fn cancel_review(&mut self, owner: ReviewOwner) -> bool {
        if self.review_owner != Some(owner) {
            return false;
        }
        self.cancel(Resource::Review);
        true
    }

    pub(crate) fn cancel_all(&mut self) {
        self.cancel(Resource::Branches);
        self.cancel(Resource::Merges);
        self.cancel(Resource::Relation);
        self.cancel(Resource::Field);
        self.cancel(Resource::Review);
    }

    pub(crate) fn loading(&self) -> bool {
        self.branches.loading
            || self.merges.loading
            || self.relation.loading
            || self.field.loading
            || self.review.loading
    }
}
