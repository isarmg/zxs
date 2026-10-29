//! Transaction-only positions in the legacy retain/push mutation buffer.
//!
//! Untouched entries inherit their base full-path position without a scan.
//! A renamed prefix redirects that origin for all untouched descendants;
//! only separately edited descendants need their own overrides. New entries
//! and file replacements append after every base entry, in mutation order.

use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Position {
    Base(String),
    Appended(usize),
}

#[derive(Default)]
pub(super) struct MutationOrder {
    origins: BTreeMap<String, Position>,
    appended: usize,
}

impl MutationOrder {
    pub(super) fn position(&self, key: &str) -> Position {
        let mut prefix = key;
        loop {
            if let Some(origin) = self.origins.get(prefix) {
                return match origin {
                    Position::Base(base) => {
                        Position::Base(format!("{base}{}", &key[prefix.len()..]))
                    }
                    Position::Appended(slot) => {
                        // All children of an appended directory were themselves
                        // appended or moved, and therefore have an exact origin.
                        assert_eq!(
                            prefix, key,
                            "invariant: a new descendant has its own origin"
                        );
                        Position::Appended(*slot)
                    }
                };
            }
            let Some((parent, _)) = prefix.rsplit_once('/') else {
                return Position::Base(key.to_owned());
            };
            prefix = parent;
        }
    }

    pub(super) fn append(&mut self, key: &str) {
        self.origins
            .insert(key.to_owned(), Position::Appended(self.appended));
        self.appended += 1;
    }

    /// Extract only overrides below this component prefix, not other edits
    /// and never the base namespace. Untouched descendants stay implicit.
    fn take_descendants(&mut self, key: &str) -> Vec<(String, Position)> {
        let prefix = format!("{key}/");
        let keys = self
            .origins
            .range(prefix.clone()..)
            .take_while(|(candidate, _)| candidate.starts_with(&prefix))
            .map(|(candidate, _)| candidate.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let origin = self
                    .origins
                    .remove(&key)
                    .expect("invariant: the selected override is present");
                (key, origin)
            })
            .collect()
    }

    pub(super) fn remove(&mut self, key: &str) {
        self.origins.remove(key);
        self.take_descendants(key);
    }

    pub(super) fn rename(&mut self, source: &str, destination: &str) {
        if source == destination {
            return;
        }
        let origin = self.position(source);
        let descendants = self.take_descendants(source);
        self.origins.remove(source);
        self.origins.insert(destination.to_owned(), origin);
        for (key, origin) in descendants {
            self.origins
                .insert(format!("{destination}{}", &key[source.len()..]), origin);
        }
    }
}
