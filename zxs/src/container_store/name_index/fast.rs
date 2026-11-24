//! Volatile certification and updates under the existing global name cut.
use super::{
    Anchor, Fence, HeldDir, Id, Limits, Live, Place, Receipt, RecycleReceipt, Replay, Required,
    Row, Store, View, active_label, capacity, codec, invalid, observe,
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
pub(in crate::container_store) struct Clock {
    generation: AtomicU64,
    outside: AtomicUsize,
    #[cfg(test)]
    catalogue_scans: AtomicUsize,
    #[cfg(test)]
    ledger_replays: AtomicUsize,
}

impl Clock {
    pub(in crate::container_store) fn fault(&self) {
        // u64::MAX permanently disables certification, not correctness.
        let _ = self
            .generation
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            });
    }

    fn mark(&self) -> Option<u64> {
        let now = self.generation.load(Ordering::Acquire);
        (now != u64::MAX).then_some(now)
    }

    fn busy(&self) -> bool {
        self.outside.load(Ordering::Acquire) != 0
    }

    fn enter(&self) -> io::Result<Background<'_>> {
        self.outside
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            })
            .map_err(|_| capacity())?;
        self.fault();
        Ok(Background(self))
    }

    #[cfg(test)]
    pub(in crate::container_store) fn count_routes(&self) {
        self.catalogue_scans.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(in crate::container_store) fn count_ledger(&self) {
        self.ledger_replays.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(in crate::container_store) fn counts(&self) -> (usize, usize) {
        (
            self.catalogue_scans.load(Ordering::Relaxed),
            self.ledger_replays.load(Ordering::Relaxed),
        )
    }
}

pub(in crate::container_store) struct Background<'a>(&'a Clock);

impl Drop for Background<'_> {
    fn drop(&mut self) {
        // Retained through every selector-stage destructor, copy arrival
        // and source retirement. The old generation remains invalid.
        self.0.outside.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::container_store) struct ChangeCut {
    route: bool,
    data: Option<[u8; 32]>,
}

impl ChangeCut {
    pub(in crate::container_store) const fn route(self) -> Self {
        Self {
            route: true,
            ..self
        }
    }

    pub(in crate::container_store) const fn data(self, token: [u8; 32]) -> Self {
        Self {
            data: Some(token),
            ..self
        }
    }
}

impl Fence {
    fn permits(&self, now: &Self, cut: ChangeCut) -> bool {
        self.same_scope(now)
            && (cut.route || self.routes == now.routes)
            && self.retired == now.retired
            && self.moves == now.moves
            && self.recycle == now.recycle
            && self
                .data
                .iter()
                .zip(&now.data)
                .all(|(before, after)| cut.data == Some(before.token) || before == after)
    }
}

pub(in crate::container_store) struct Ticket {
    mark: Option<u64>,
    fence: Option<Fence>,
    required: Option<Required>,
    reusable: bool,
}

impl Ticket {
    pub(in crate::container_store) fn held(&self) -> bool {
        self.fence.is_some()
    }

    fn certified(&self) -> bool {
        self.held() && self.required.is_none()
    }

    pub(in crate::container_store) fn require_generation(&self, store: &Store) -> io::Result<()> {
        if store.name_clock.mark() == self.mark {
            Ok(())
        } else {
            Err(invalid("observed name authority changed"))
        }
    }

    fn observe(&self, store: &Store, cut: ChangeCut) -> io::Result<Option<Fence>> {
        self.require_generation(store)?;
        let Some(original) = self.fence.as_ref() else {
            return Ok(None);
        };
        let system = store
            .archive
            .as_ref()
            .ok_or_else(|| invalid("selected RECYCLE is absent"))?
            .indexed_system();
        let current = Fence::sample(store, system, self.required.as_ref())?;
        if original.permits(&current, cut) && store.name_clock.mark() == self.mark {
            Ok(Some(current))
        } else {
            Err(invalid("name authority changed before the native cut"))
        }
    }

    pub(in crate::container_store) fn require(
        &self,
        store: &Store,
        cut: ChangeCut,
    ) -> io::Result<()> {
        store.name_observed(self.observe(store, cut).map(|_| ()))
    }
}

#[derive(Default)]
pub(in crate::container_store) struct Names {
    prior: Option<View>,
    ready: Option<u64>,
    retention_failed: bool,
}

impl Names {
    pub(in crate::container_store) fn enter_background<'a>(
        &mut self,
        clock: &'a Clock,
    ) -> io::Result<Background<'a>> {
        self.invalidate();
        clock.enter()
    }

    pub(in crate::container_store) fn invalidate(&mut self) {
        self.ready = None;
    }

    pub(in crate::container_store) fn admit(
        &mut self,
        store: &Store,
        except: Id,
        label: &str,
        target: [u8; 32],
    ) -> io::Result<Option<Ticket>> {
        let mark = store.name_clock.mark();
        let Some(system) = store
            .archive
            .as_ref()
            .map(super::super::recycle::Recycle::indexed_system)
        else {
            let available = store.vacant(except, label)?;
            return Ok(available.then_some(Ticket {
                mark,
                fence: None,
                required: None,
                reusable: false,
            }));
        };
        if self.retention_failed {
            return Err(invalid(
                "a confirmed name could not retain its native witness",
            ));
        }
        let idle = !store.name_clock.busy();
        if idle
            && mark.is_some()
            && self.ready == mark
            && let Some(known) = self.prior.as_ref()
            && let Ok(fence) = Fence::capture(store, system)
            && Some(&fence) == known.fence.as_ref()
            && store.name_clock.mark() == mark
        {
            return Ok(known.vacant(except, label).then_some(Ticket {
                mark,
                fence: Some(fence),
                required: None,
                reusable: true,
            }));
        }
        self.invalidate();
        let result = View::scan(
            store,
            system,
            self.prior.as_ref(),
            &Limits::default(),
            || Ok(()),
        );
        match result {
            Ok(current) if store.name_clock.mark() == mark => {
                let reusable = idle && mark.is_some() && !store.name_clock.busy();
                let ticket = current.vacant(except, label).then(|| Ticket {
                    mark,
                    fence: current.fence.clone(),
                    required: None,
                    reusable,
                });
                self.prior = Some(current);
                self.ready = mark.filter(|_| reusable);
                Ok(ticket)
            }
            // Exhaustion, a missing irrelevant volume, or an observed
            // unconfirmed cut cannot justify using a stale cached negative.
            Ok(_) => Err(invalid("name authority changed during re-qualification")),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotConnected
                        | io::ErrorKind::FileTooLarge
                        | io::ErrorKind::WouldBlock
                ) =>
            {
                self.exact(store, system, except, (label, target), mark)
            }
            Err(error) => Err(error),
        }
    }

    fn exact(
        &self,
        store: &Store,
        system: &HeldDir,
        except: Id,
        (label, target): (&str, [u8; 32]),
        mark: Option<u64>,
    ) -> io::Result<Option<Ticket>> {
        let required = Self::required_roles(store, target)?;
        let fence = Fence::sample(store, system, Some(&required))?;
        // Do not silently forget any selected state already witnessed.
        if let Some(prior) = &self.prior {
            let limits = Limits::default();
            for (&id, row) in &prior.rows {
                let token = store
                    .routes
                    .get(id)?
                    .ok_or_else(|| invalid("a previously observed route is absent"))?;
                observe(store, id, token, Some(row), &limits)?;
            }
        }
        let available = store.vacant(except, label)?;
        if Fence::sample(store, system, Some(&required))? != fence
            || store.name_clock.mark() != mark
        {
            return Err(invalid("name authority changed in the exact fallback"));
        }
        Ok(available.then_some(Ticket {
            mark,
            fence: Some(fence),
            required: Some(required),
            reusable: false,
        }))
    }

    fn required_roles(store: &Store, target: [u8; 32]) -> io::Result<Required> {
        let mut scope = Required::default();
        scope.data.insert(target);
        #[cfg(test)]
        store.name_clock.count_routes();
        for id in store.name_observed(store.routes.ids())? {
            let token = store
                .name_observed(store.routes.get(id))?
                .ok_or_else(|| invalid("a routed name lost its original DATA token"))?;
            if store.recycled(id)?.is_some() {
                scope.recycle = true;
            } else {
                store.enrolled(token).ok_or_else(super::unenrolled_data)?;
                scope.data.insert(token);
            }
        }
        Ok(scope)
    }

    pub(in crate::container_store) fn recycled(
        &mut self,
        receipt: RecycleReceipt,
        at: &HeldDir,
        token: [u8; 32],
    ) {
        self.invalidate();
        let known = self.prior.get_or_insert_with(View::partial);
        if known.record_recycled(receipt, at, token).is_err() {
            self.retention_failed = true;
        }
    }

    pub(in crate::container_store) fn confirm(
        &mut self,
        store: &Store,
        ticket: &Ticket,
        cut: ChangeCut,
        receipt: &Receipt,
        original: (&HeldDir, [u8; 32]),
    ) {
        if store.archive.is_none() {
            return; // The standalone legacy test backend has no name index.
        }
        let known = self.prior.get_or_insert_with(View::partial);
        // The reply is already durable. Keep that exact history even after
        // an unindexed fallback; a later lost child is not a reservation.
        if known.record(receipt, original, ticket.certified()).is_err() {
            self.retention_failed = true;
            return;
        }
        if ticket.reusable
            && !store.name_clock.busy()
            && let Ok(Some(current)) = ticket.observe(store, cut)
        {
            known.fence = Some(current);
            self.ready = ticket.mark;
        }
    }
}

impl View {
    fn record(
        &mut self,
        receipt: &Receipt,
        (directory, token): (&HeldDir, [u8; 32]),
        certified: bool,
    ) -> io::Result<()> {
        let id = receipt.mutation.container;
        let name = active_label(receipt).ok_or_else(|| invalid("not a display-name receipt"))?;
        let anchor = Anchor::new(receipt)?;
        let at = Live::capture(directory)?;
        let old = self.rows.get(&id);
        let (prior_ledger, old_label) = match old {
            Some(row) if row.token == token => match &row.place {
                Place::Active {
                    name,
                    at: prior,
                    ledger,
                    warm,
                } if *prior == at
                    && ledger.is_none_or(|old| receipt.sequence > old.sequence)
                    && warm.is_none_or(|old| receipt.sequence > old.sequence) =>
                {
                    (*ledger, Some(Arc::clone(name)))
                }
                Place::Reserved(original) if original.is_none_or(|held| held == at) => (None, None),
                _ => {
                    return Err(invalid(
                        "confirmed name differs from its retained disposition",
                    ));
                }
            },
            None if self.rows.len() < Limits::default().rows => (None, None),
            _ => return Err(capacity()),
        };
        let ledger = match (prior_ledger, receipt.sequence) {
            (Some(prior), _) => Some(prior),
            (None, 1) => {
                let mut first = Replay::new(id, None, 0)?;
                first.receipt(1, &codec::encode(receipt)?)?;
                Some(super::Ledger {
                    sequence: 1,
                    digest: first.digest,
                })
            }
            (None, _) if !certified => None,
            (None, _) => return Err(invalid("new indexed name has no first sequence")),
        };
        let label_bytes = self.label_bytes - old_label.as_ref().map_or(0, |value| value.len());
        let label_bytes = label_bytes
            .checked_add(name.len())
            .filter(|&total| total <= Limits::default().labels)
            .ok_or_else(capacity)?;
        if certified && !self.vacant(id, name) {
            return Err(invalid("confirmed name conflicts with retained name map"));
        }
        if let Some(previous) = old_label
            && self.names.get(previous.as_ref()) == Some(&id)
        {
            self.names.remove(previous.as_ref());
        }
        let name = Arc::<str>::from(name);
        self.names.insert(Arc::clone(&name), id);
        self.rows.insert(
            id,
            Row {
                token,
                place: Place::Active {
                    name,
                    at,
                    ledger,
                    warm: Some(anchor),
                },
            },
        );
        self.label_bytes = label_bytes;
        Ok(())
    }

    fn record_recycled(
        &mut self,
        receipt: RecycleReceipt,
        directory: &HeldDir,
        token: [u8; 32],
    ) -> io::Result<()> {
        let id = receipt.mutation.container;
        let old = self.rows.get(&id);
        let previous = old.map(|row| &row.place);
        if old.is_some_and(|row| row.token != token)
            || previous.is_none() && self.rows.len() >= Limits::default().rows
        {
            return Err(capacity());
        }
        let place = Place::Recycled {
            receipt,
            at: Live::capture(directory)?,
            ledger: previous.and_then(Place::ledger),
            warm: previous.and_then(Place::anchor),
        };
        if let Some(previous) = previous {
            place.require_progress(previous, token)?;
            if let Place::Active { name, .. } = previous {
                self.label_bytes -= name.len();
                if self.names.get(name.as_ref()) == Some(&id) {
                    self.names.remove(name.as_ref());
                }
            }
        }
        self.rows.insert(id, Row { token, place });
        Ok(())
    }
}
