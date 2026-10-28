//! Per-frame contour work without browser types: the explicit invalidation
//! set, the author-transition policy and the read/write phase order.
//!
//! The controller merges pending childList records into the registry, takes
//! the invalidation set, runs [`TRANSITION_RECORD`] for each invalidated
//! record in its existing order and then executes [`FRAME`] in order. Every step
//! either only reads browser style/layout or only writes it (see
//! [`Step::Qualify`] for the single retained exception), so transition
//! policy preserves its original inter-record dependencies while foreground
//! positioning uses separate read/write batches.
use super::paint::{Separator, number, owned_paint_property, split};

/// Something that requires a later contour frame.
pub(super) enum Trigger<N> {
    /// childList records changed the registered HTML.
    Structure,
    /// Author attribute/text changes, author writes observed during paint, or
    /// work deferred while a paint was running.
    Author,
    /// An interaction event: its element target followed by every ancestor.
    Interaction(Vec<N>),
    /// Transition/animation edges, resize, media, fonts, active extent
    /// transitions or an explicit refresh. Author transition policy is kept.
    Passive,
}

/// Which records must re-derive their transition overrides this frame.
#[derive(Debug, PartialEq)]
pub(super) enum TransitionScope<N> {
    Unchanged,
    Chains(Vec<N>),
    All,
}

/// Work accumulated since the last frame. Qualification, paint and
/// foreground positioning always cover every registered node: `:has()`,
/// `:dir()`, sibling combinators, container queries and inherited extents
/// change nodes that received no event, so no trigger narrows them.
#[derive(Debug)]
pub(super) struct Invalidation<N> {
    transitions: TransitionScope<N>,
}

impl<N> Default for Invalidation<N> {
    fn default() -> Self {
        Self {
            transitions: TransitionScope::Unchanged,
        }
    }
}

/// The work one frame takes from the invalidation set. Registry changes are
/// merged before the frame; everything else covers all registered nodes.
#[derive(Debug, PartialEq)]
pub(super) struct FrameWork<N> {
    pub transitions: TransitionScope<N>,
}

impl<N: PartialEq> Invalidation<N> {
    pub(super) fn add(&mut self, trigger: Trigger<N>) {
        match trigger {
            Trigger::Structure | Trigger::Author => self.transitions = TransitionScope::All,
            Trigger::Interaction(chain) => match &mut self.transitions {
                TransitionScope::All => {}
                TransitionScope::Chains(known) => {
                    for node in chain {
                        if !known.contains(&node) {
                            known.push(node);
                        }
                    }
                }
                scope @ TransitionScope::Unchanged => *scope = TransitionScope::Chains(chain),
            },
            Trigger::Passive => {}
        }
    }

    pub(super) fn take(&mut self) -> FrameWork<N> {
        FrameWork {
            transitions: std::mem::take(self).transitions,
        }
    }
}

/// Phases of one contour frame, executed in this order by the controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Step {
    /// Write back the author transition declarations of invalidated records.
    TransitionRestore,
    /// Read their computed transition lists.
    TransitionRead,
    /// Write their overrides that exclude controller-owned paint properties.
    TransitionWrite,
    /// Restore every record's author paint.
    PaintRestore,
    /// Read qualification, snapshots, sizes and fragments of all registered
    /// nodes in document order. The one exception to read-only: a first-seen
    /// candidate with an owned-paint transition has its override written
    /// before its own snapshot, as it always was, because that write can
    /// cancel a running transition that later nodes inherit from.
    Qualify,
    /// Paint and commit every candidate.
    Paint,
    /// Read every visible foreground's screen matrix and host rectangle.
    ForegroundRead,
    /// Write every foreground's corrected offset.
    ForegroundWrite,
    /// Read them again: inside a transformed scroll container a moved
    /// foreground can change scrollable overflow and so other hosts.
    ForegroundVerify,
    /// Write only the foregrounds whose offset changed.
    ForegroundCorrect,
}

/// Transition policy steps, completed for each invalidated record before the
/// next record and before [`FRAME`]. Even siblings can depend on each other's
/// inline style through selectors, so nesting does not prove independence.
pub(super) const TRANSITION_RECORD: [Step; 3] = [
    Step::TransitionRestore,
    Step::TransitionRead,
    Step::TransitionWrite,
];

pub(super) const FRAME: [Step; 7] = [
    Step::PaintRestore,
    Step::Qualify,
    Step::Paint,
    Step::ForegroundRead,
    Step::ForegroundWrite,
    Step::ForegroundVerify,
    Step::ForegroundCorrect,
];

/// Computed `transition-*` lists of one element, in declaration order.
pub(super) struct TransitionLists {
    pub properties: Vec<String>,
    /// duration, delay, timing function and behavior.
    pub values: [Vec<String>; 4],
}

impl TransitionLists {
    pub(super) fn parse(properties: &str, values: [&str; 4]) -> Self {
        Self {
            properties: split(properties, Separator::Comma),
            values: values.map(|value| split(value, Separator::Comma)),
        }
    }
}

/// The inline `transition-property`, `-duration`, `-delay`,
/// `-timing-function` and `-behavior` declarations that keep every author transition
/// except those of controller-owned paint properties. `None` when no owned
/// paint property (directly or through `all`) has a positive duration, in
/// which case the author declarations are left in place.
pub(super) fn transition_overrides(
    lists: &TransitionLists,
    all_properties: &[String],
) -> Option<[String; 5]> {
    let at = |list: &[String], index: usize| {
        if list.is_empty() {
            String::new()
        } else {
            list[index % list.len()].clone()
        }
    };
    if !lists.properties.iter().enumerate().any(|(index, name)| {
        (name == "all" || owned_paint_property(name)) && number(&at(&lists.values[0], index)) > 0.0
    }) {
        return None;
    }
    let mut retained = Vec::new();
    for (index, name) in lists.properties.iter().enumerate() {
        if name == "all" {
            retained.extend(all_properties.iter().map(|name| (name.clone(), index)));
        } else if !owned_paint_property(name) && name != "none" {
            retained.push((name.clone(), index));
        }
    }
    let names = retained
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let mut result: [String; 5] = Default::default();
    result[0] = if names.is_empty() {
        "none".into()
    } else {
        names
    };
    for (index, list) in lists.values.iter().enumerate() {
        let fallback = match index {
            2 => "ease",
            3 => "normal",
            _ => "0s",
        };
        let value = retained
            .iter()
            .map(|(_, source)| {
                let value = at(list, *source);
                if !value.is_empty() {
                    value
                } else if index == 3 {
                    "normal".to_owned()
                } else {
                    "0s".to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        result[index + 1] = if value.is_empty() {
            fallback.into()
        } else {
            value
        };
    }
    Some(result)
}

#[cfg(test)]
#[path = "../../../../test/interface/unit/contours/frame/tests.rs"]
mod tests;
