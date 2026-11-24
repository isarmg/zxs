//! One native clipboard request, scoped to its exact editor or login challenge.
use std::time::{Duration, Instant};

use eframe::egui::{self, Context, Event, Key, RawInput};
use zeroize::Zeroize;

use crate::actor::Claim;

#[derive(Clone)]
pub enum Target {
    Ordinary(egui::Id),
    Protected(Claim),
}

impl Target {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ordinary(a), Self::Ordinary(b)) => a == b,
            (Self::Protected(a), Self::Protected(b)) => a.same(b),
            _ => false,
        }
    }
}

struct Pending {
    target: Option<Target>,
    deadline: Instant,
    first_paint: u64,
}

#[derive(Default)]
pub struct Queue {
    pending: Option<Pending>,
    paints: u64,
    last_frame: Option<u64>,
}

impl Queue {
    pub fn request(&mut self, ctx: &Context, target: Target) {
        if self.pending.is_none() {
            self.arm(target, Instant::now());
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
    }

    fn arm(&mut self, target: Target, now: Instant) {
        self.pending = Some(Pending {
            target: Some(target),
            deadline: now + Duration::from_secs(2),
            first_paint: self.paints,
        });
    }

    pub fn painted(&mut self, ctx: &Context, raw_frame: u64) {
        if self.last_frame != Some(raw_frame) {
            self.last_frame = Some(raw_frame);
            self.paints = self.paints.wrapping_add(1);
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(25));
        }
    }

    pub fn waiting_for(&self, target: &Target) -> bool {
        self.pending
            .as_ref()
            .and_then(|p| p.target.as_ref())
            .is_some_and(|p| p.same(target))
    }

    pub fn receive(&mut self, raw: &mut RawInput, active: Option<&Target>, now: Instant) -> bool {
        let moved = matches!(active, Some(Target::Ordinary(_)))
            && raw.events.iter().any(|event| {
                matches!(
                    event,
                    Event::PointerButton { .. }
                        | Event::Key {
                            key: Key::Tab | Key::Enter,
                            pressed: true,
                            ..
                        }
                )
            });
        let matching = active.is_some_and(|target| self.waiting_for(target));
        let mut admit =
            matching && !moved && self.pending.as_ref().is_some_and(|p| now < p.deadline);
        let mut delivered = false;
        raw.events.retain_mut(|event| match event {
            Event::Paste(_) if admit => {
                admit = false;
                delivered = true;
                true
            }
            Event::Paste(text) => {
                text.zeroize();
                delivered = true;
                false
            }
            Event::PasteImage(_) => {
                delivered = true;
                false
            }
            _ => true,
        });
        let expired = self
            .pending
            .as_ref()
            .is_some_and(|p| now >= p.deadline && self.paints.wrapping_sub(p.first_paint) >= 2);
        if delivered || expired {
            self.pending = None;
        } else if (!matching || moved)
            && let Some(pending) = &mut self.pending
        {
            // No ABA on refocus. Do not issue a second request until the
            // first result arrives or two visible passes have drained it.
            pending.target = None;
        }
        expired && matching && !moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_paste_keeps_its_target_through_glows_empty_pass() {
        let mut clipboard = Queue::default();
        let a = Target::Ordinary(egui::Id::unique("A"));
        let b = Target::Ordinary(egui::Id::unique("B"));
        let now = Instant::now();
        let mut raw = RawInput::default();
        clipboard.arm(a.clone(), now);
        assert!(!clipboard.receive(&mut raw, Some(&a), now));
        assert!(clipboard.waiting_for(&a));
        raw.events = vec![
            Event::Paste("first text".into()),
            Event::Paste("duplicate".into()),
        ];
        assert!(!clipboard.receive(&mut raw, Some(&a), now));
        assert_eq!(raw.events, vec![Event::Paste("first text".into())]);

        raw.events.clear();
        clipboard.arm(a.clone(), now);
        clipboard.receive(&mut raw, None, now);
        assert!(clipboard.pending.is_some() && !clipboard.waiting_for(&a));
        clipboard.receive(&mut raw, Some(&a), now);
        assert!(!clipboard.waiting_for(&a));
        raw.events.push(Event::Paste("late".into()));
        clipboard.receive(&mut raw, Some(&a), now);
        assert!(raw.events.is_empty());

        let first = Target::Protected(Claim::new());
        let second = Target::Protected(Claim::new());
        clipboard.arm(first.clone(), now);
        raw.events.push(Event::Paste("other challenge".into()));
        clipboard.receive(&mut raw, Some(&second), now);
        assert!(raw.events.is_empty());
        clipboard.arm(first.clone(), now);
        raw.events.push(Event::Paste("ordinary lane".into()));
        clipboard.receive(&mut raw, Some(&b), now);
        assert!(raw.events.is_empty());
        clipboard.arm(first.clone(), now);
        let enter = Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        raw.events.push(Event::Paste("protected text".into()));
        raw.events.push(enter.clone());
        clipboard.receive(&mut raw, Some(&first), now);
        assert_eq!(
            raw.events,
            vec![Event::Paste("protected text".into()), enter]
        );
    }
}
