//! Stable-key page selection retains at most the admitted record budget.
use super::*;

pub(super) enum Position<K> {
    Start,
    After(K),
    Before(K),
    At(K),
}

pub(super) struct Selection<K, T> {
    pub(super) entries: Vec<(K, T)>,
    pub(super) has_before: bool,
    pub(super) has_after: bool,
}

pub(super) struct Selector<K, T> {
    position: Position<K>,
    limit: usize,
    entries: BTreeMap<K, T>,
    boundary_seen: bool,
    before_anchor: bool,
    overflow: bool,
}

impl<K: Ord, T> Selector<K, T> {
    pub(super) fn new(position: Position<K>, limit: usize) -> Result<Self> {
        if limit == 0 {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "history page record budget must permit progress",
            ));
        }
        Ok(Self {
            position,
            limit,
            entries: BTreeMap::new(),
            boundary_seen: false,
            before_anchor: false,
            overflow: false,
        })
    }

    pub(super) fn offer(&mut self, key: K, value: T) {
        let eligible = match &self.position {
            Position::Start => true,
            Position::After(boundary) | Position::Before(boundary) | Position::At(boundary) => {
                self.boundary_seen |= &key == boundary;
                self.before_anchor |= &key < boundary;
                match &self.position {
                    Position::After(_) => &key > boundary,
                    Position::Before(_) => &key < boundary,
                    Position::At(_) => &key >= boundary,
                    Position::Start => unreachable!("invariant: position arm is a boundary"),
                }
            }
        };
        if !eligible {
            return;
        }
        self.entries.insert(key, value);
        if self.entries.len() > self.limit {
            self.overflow = true;
            if matches!(self.position, Position::Before(_)) {
                self.entries.pop_first();
            } else {
                self.entries.pop_last();
            }
        }
    }

    pub(super) fn finish(self) -> Result<Selection<K, T>> {
        if !matches!(self.position, Position::Start) && !self.boundary_seen {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "history page boundary is outside the allowed collection",
            ));
        }
        let (has_before, has_after) = match self.position {
            Position::Start => (false, self.overflow),
            Position::After(_) => (true, self.overflow),
            Position::Before(_) => (self.overflow, true),
            Position::At(_) => (self.before_anchor, self.overflow),
        };
        Ok(Selection {
            entries: self.entries.into_iter().collect(),
            has_before,
            has_after,
        })
    }
}
