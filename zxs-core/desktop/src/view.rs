//! One window with explicit typed actions; no networking or SQLite on the UI thread.
use std::{path::PathBuf, time::Instant};

use eframe::egui::{self, Context, PointerButton, RawInput, Ui};
use tokio::sync::{mpsc, watch};
use zxs_core::{
    Result,
    r3::{Branch, Lookup},
};

use crate::{
    actor::{self, Action, Card, Claim, Effect, Message, Snapshot},
    secret::{Password, native_interrupted},
};

enum Gate {
    Idle,
    Review,
    Verifying(Claim),
    Prompt { claim: Claim, shown: bool },
    WaitingForCatalog(Claim),
    Retiring,
}

impl Gate {
    fn claim(&self) -> Option<&Claim> {
        match self {
            Self::Verifying(claim)
            | Self::Prompt { claim, .. }
            | Self::WaitingForCatalog(claim) => Some(claim),
            _ => None,
        }
    }
}

#[derive(Default)]
struct ApprovalFrames {
    native: u64,
    admitted: bool,
    seen: Option<(u64, Claim)>,
}

impl ApprovalFrames {
    /// Called before the toolkit runs logic or lays out any widget.
    fn begin(&mut self, admitted: bool, existing: Option<&Claim>) {
        self.native = self.native.wrapping_add(1);
        self.admitted = admitted;
        if !admitted {
            if let Some(existing) = existing {
                existing.revoke();
            }
            self.seen = None;
        }
    }

    fn show(&mut self, claim: &Claim) -> bool {
        if !self.admitted || !claim.live() {
            self.seen = None;
            return false;
        }
        if let Some((frame, previous)) = &self.seen
            && previous.same(claim)
        {
            // Re-layout during the same native frame is not prior visibility.
            return *frame != self.native;
        }
        self.seen = Some((self.native, claim.clone()));
        false
    }
}

pub struct Desktop {
    to_owner: mpsc::Sender<Message>,
    updates: watch::Receiver<Snapshot>,
    view: Snapshot,
    waiting: bool,
    approved_send: bool,
    approvals: ApprovalFrames,
    gate: Gate,
    password: Password,
    rejected_input: bool,
    state_path: String,
    origin: String,
    account: String,
    destination: String,
    branch: Option<Branch>,
    working: String,
}

impl Desktop {
    pub fn new(ctx: Context) -> Result<Self> {
        let (to_owner, updates) = actor::start(ctx)?;
        Ok(Self {
            to_owner,
            updates,
            view: Snapshot::default(),
            waiting: false,
            approved_send: false,
            approvals: ApprovalFrames::default(),
            gate: Gate::Idle,
            password: Password::default(),
            rejected_input: false,
            state_path: String::new(),
            origin: String::new(),
            account: String::new(),
            destination: String::new(),
            branch: None,
            working: String::new(),
        })
    }

    fn send(&mut self, action: Action) -> bool {
        if self.waiting
            || self
                .to_owner
                .try_send(Message(self.view.epoch, action))
                .is_err()
        {
            return false;
        }
        self.waiting = true;
        true
    }

    fn retire(&mut self) {
        if let Some(claim) = self.gate.claim() {
            claim.revoke();
        }
        self.password.clear();
        self.rejected_input = false;
        self.gate = Gate::Retiring;
    }

    fn update_view(&mut self) {
        match self.updates.has_changed() {
            Ok(false) => return,
            Ok(true) => {}
            Err(_) => {
                self.waiting = false;
                self.approved_send = false;
                self.view.connected = false;
                self.view.card = None;
                self.view.notice = "The workspace worker stopped. No new result is known. Restart to check original receipts.";
                if !matches!(self.gate, Gate::Idle | Gate::Retiring) {
                    self.retire();
                }
                return;
            }
        }
        let next = self.updates.borrow_and_update().clone();
        if next.working != self.view.working
            && let Some(p) = &next.working
        {
            self.working = p.display().to_string();
        }
        if next.detail != self.view.detail {
            self.branch = None;
        }
        if next.epoch != self.view.epoch {
            self.approvals.seen = None;
            if !matches!(self.gate, Gate::Idle) {
                self.retire();
            }
            self.destination.clear();
            self.working.clear();
        }
        if next.serial != self.view.serial {
            self.waiting = false;
            self.approved_send = false;
        }
        self.view = next;
        match &self.gate {
            Gate::Verifying(original) if !self.waiting => {
                if self
                    .view
                    .prompt
                    .as_ref()
                    .is_some_and(|ready| ready.claim.same(original) && original.live())
                {
                    self.gate = Gate::Prompt {
                        claim: original.clone(),
                        shown: false,
                    };
                } else {
                    self.retire();
                }
            }
            Gate::Prompt { claim, .. }
                if !self.view.prompt.as_ref().is_some_and(|ready| {
                    ready.claim.same(claim) && claim.live() && Instant::now() < ready.deadline
                }) =>
            {
                self.retire()
            }
            Gate::WaitingForCatalog(_) if !self.waiting => self.retire(),
            _ => {}
        }
    }

    fn protective_panel(&mut self, ui: &mut Ui) {
        match &self.gate {
            Gate::Review => {
                ui.heading("Review the original connection");
                if let Some(path) = &self.view.path {
                    ui.label(path.display().to_string());
                }
                ui.label(format!("HTTPS origin: {}", self.origin));
                ui.label(format!("Account: {}", self.account));
                ui.label("The selected workspace and HTTPS/2 must be verified before any password prompt.");
                if ui.button("Cancel").clicked() {
                    self.retire();
                } else if ui
                    .button("Verify HTTPS/2")
                    .clicked_by(PointerButton::Primary)
                {
                    let claim = Claim::new();
                    if self.send(Action::Begin {
                        origin: self.origin.clone(),
                        account: self.account.clone(),
                        claim: claim.clone(),
                    }) {
                        self.gate = Gate::Verifying(claim);
                    }
                }
            }
            Gate::Verifying(_) => {
                ui.spinner();
                ui.label("Checking the original workspace, certificate, hostname and HTTP/2.");
                if ui.button("Cancel verification").clicked() {
                    self.retire();
                }
            }
            Gate::Prompt { claim, shown } => {
                let claim = claim.clone();
                let was_shown = *shown;
                let Some(ready) = self.view.prompt.as_ref().filter(|ready| {
                    ready.claim.same(&claim) && claim.live() && Instant::now() < ready.deadline
                }) else {
                    self.retire();
                    return;
                };
                ui.heading("Protected password entry");
                ui.label(format!("Verified HTTPS origin: {}", ready.authority));
                ui.label(format!("Account: {}", ready.account));
                ui.label("Type directly. Paste, IME and password reveal are not supported.");
                ui.label(if self.password.has_value() {
                    "Password entered (hidden)"
                } else {
                    "No password entered"
                });
                if self.rejected_input {
                    ui.label("Input was discarded. Retype directly; do not paste.");
                }
                if ui.button("Cancel").clicked() {
                    self.retire();
                } else if ui
                    .add_enabled(
                        was_shown && self.password.has_value() && !self.waiting,
                        egui::Button::new("Sign in to this exact workspace"),
                    )
                    .clicked_by(PointerButton::Primary)
                    && let Some(password) = self.password.take()
                {
                    if self.send(Action::Finish {
                        claim: claim.clone(),
                        password,
                    }) {
                        self.gate = Gate::WaitingForCatalog(claim);
                    } else {
                        self.rejected_input = true;
                    }
                } else if let Gate::Prompt { shown, .. } = &mut self.gate {
                    *shown = true;
                }
            }
            Gate::WaitingForCatalog(claim) => {
                ui.spinner();
                if claim.live() {
                    ui.label("Waiting for the verified channel. You can still cancel before credentials are sent.");
                    if ui
                        .button("Cancel sign-in")
                        .clicked_by(PointerButton::Primary)
                        && claim.revoke()
                    {
                        self.retire();
                    }
                } else if claim.accepted() {
                    ui.label("The first request was admitted. Waiting for the authorized catalog.");
                } else {
                    ui.label("Sign-in was canceled before sending.");
                }
            }
            Gate::Retiring => {
                ui.label(self.view.notice);
                if !self.waiting {
                    self.gate = Gate::Idle;
                    ui.ctx().request_repaint();
                }
            }
            Gate::Idle => {}
        }
    }

    fn confirm(&mut self, ui: &mut Ui, card: &Card, previously_shown: bool) {
        ui.heading(match card.effect {
            Effect::Checkout => "Confirm exact checkout",
            Effect::Freeze => "Freeze this exact full file locally",
            Effect::Send => "Submit this original operation",
        });
        ui.label(format!("File: {}", card.path));
        ui.label(format!("Container: {}", card.basis.container));
        ui.label(format!("Branch: {}", card.basis.branch));
        ui.label(format!("Approved head: {}", card.basis.node));
        ui.label(format!("Container revision: {}", card.basis.seen_sequence));
        ui.label(format!("Full body: {} bytes", card.body.length));
        ui.label(format!("SHA-256: {}", card.body.sha256));
        if let Some(intent) = &card.intent {
            ui.label(format!("Operation: {}", intent.operation_id));
            ui.label(format!("New node: {}", intent.node_id));
            ui.label("Only authoritative absence of this exact receipt permits sending these frozen bytes.");
        }
        if card.claim.revoked() {
            ui.label("This choice was canceled before it was accepted. Close it and review again.");
        }
        if ui
            .add_enabled(!self.waiting, egui::Button::new("Close choice"))
            .clicked()
        {
            card.claim.revoke();
            self.send(Action::Dismiss);
        } else if ui
            .add_enabled(
                !self.waiting && previously_shown && card.claim.live(),
                egui::Button::new("Confirm this exact choice"),
            )
            .clicked_by(PointerButton::Primary)
            && self.send(Action::Approve(card.claim.clone()))
        {
            self.approved_send = matches!(card.effect, Effect::Send);
        }
        if self.approved_send {
            ui.label(
                "Submit in progress. If interrupted, keep the original and check its receipt.",
            );
        }
    }

    fn normal(&mut self, ui: &mut Ui) {
        let mut action = None;
        ui.heading("ZXS Desktop");
        ui.label(self.view.notice);
        if self.waiting {
            ui.spinner();
        }
        if let Some(card) = self.view.card.clone() {
            let shown = self.approvals.show(&card.claim);
            ui.push_id(("exact-choice", card.claim.widget_scope()), |ui| {
                self.confirm(ui, &card, shown);
            });
            if !shown && self.approvals.admitted && card.claim.live() {
                ui.ctx().request_repaint();
            }
            return;
        }
        self.approvals.seen = None;
        ui.add_enabled_ui(!self.waiting, |ui| {
            if let Some(path) = &self.view.path {
                ui.label(format!("Original private workspace: {}", path.display()));
            }
            ui.horizontal(|ui| {
                ui.label("Existing state directory:");
                ui.text_edit_singleline(&mut self.state_path);
                if ui.button("Open existing").clicked() && !self.state_path.is_empty() {
                    action = Some(Action::Open(PathBuf::from(&self.state_path)));
                }
            });
            if !self.view.connected {
                ui.label("This app opens existing enrolled workspaces. It does not create a new one.");
                ui.horizontal(|ui| {
                    ui.label("Original HTTPS origin");
                    ui.text_edit_singleline(&mut self.origin);
                    ui.label("Account");
                    ui.text_edit_singleline(&mut self.account);
                    if ui.add_enabled(self.view.path.is_some() && !self.origin.is_empty() && !self.account.is_empty(),
                        egui::Button::new("Review connection")).clicked() {
                        // The next frame is read-only and scrubbed, before Verify.
                        self.password.clear();
                        ui.ctx().memory_mut(|memory| {
                            if let Some(editable) = memory.focused() {
                                memory.surrender_focus(editable);
                            }
                        });
                        self.gate = Gate::Review;
                        ui.ctx().request_repaint();
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(format!("Authorized: {} · {}", self.view.authority, self.view.account));
                    if ui.button("Disconnect").clicked() {
                        action = Some(Action::Disconnect);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("First catalog page").clicked() { action = Some(Action::Catalog { more: false }); }
                    if ui.add_enabled(self.view.after.is_some(), egui::Button::new("Next page")).clicked() {
                        action = Some(Action::Catalog { more: true });
                    }
                });
                egui::ScrollArea::vertical().id_salt("catalog").max_height(150.0).show(ui, |ui| {
                    for row in &self.view.rows {
                        if ui.button(format!("{} [{}]", row.name, row.container_id)).clicked() {
                            action = Some(Action::Detail(row.container_id.clone()));
                        }
                    }
                });
                if let Some(detail) = &self.view.detail {
                    ui.label(format!("Selected container: {} / {}", detail.name, detail.container_id));
                    egui::ScrollArea::vertical().id_salt("branches").max_height(100.0).show(ui, |ui| {
                        for item in &detail.heads {
                            if ui.selectable_label(self.branch.as_ref() == Some(&item.branch),
                                format!("{} [{}]", item.branch, item.head)).clicked() {
                                self.branch = Some(item.branch.clone());
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Exact new ordinary file:");
                        ui.text_edit_singleline(&mut self.destination);
                        let head = self.branch.as_ref().and_then(|b| detail.heads.iter().find(|h| &h.branch == b));
                        if ui.add_enabled(head.is_some() && !self.destination.is_empty(), egui::Button::new("Review checkout")).clicked()
                            && let Some(head) = head {
                            action = Some(Action::Choose {
                                container: detail.container_id.clone(), branch: head.branch.clone(),
                                node: head.head.clone(), output: self.destination.as_str().into(),
                            });
                        }
                    });
                }
            }
            if self.view.path.is_some() {
                ui.separator();
                ui.label("Edit the ordinary path in your preferred application. Editor saves never submit.");
                ui.horizontal(|ui| {
                    ui.label("Mapped working file:");
                    ui.text_edit_singleline(&mut self.working);
                    if ui.add_enabled(!self.working.is_empty() && !self.view.fresh_send, egui::Button::new("Review full change")).clicked() {
                        action = Some(Action::Review(self.working.as_str().into()));
                    }
                });
                if ui.add_enabled(self.view.connected && self.view.fresh_send, egui::Button::new("Review newly frozen original for submit")).clicked() {
                    action = Some(Action::ReviewSend);
                }
                ui.heading("Pending originals");
                if let Some(result) = &self.view.resolution {
                    ui.label(format!("Original operation: {}", result.operation));
                    ui.label(match result.status {
                        Lookup::Confirmed { .. } => "Exact receipt confirmed.",
                        Lookup::Absent => "The original receipt is absent. No automatic submit.",
                        Lookup::Conflict => "The original has a conflict. Check its receipt; do not resend.",
                        Lookup::Unknown => "The original result is unknown. Keep the same operation.",
                    });
                    if result.local_cleanup_pending { ui.label("Confirmed receipt; local cleanup still pending."); }
                }
                if self.view.pending_more { ui.label("Only the first 32 local pending originals are displayed."); }
                egui::ScrollArea::vertical().id_salt("pending").max_height(120.0).show(ui, |ui| {
                    for op in &self.view.pending {
                        ui.horizontal(|ui| {
                            ui.label(op.as_str());
                            if ui.add_enabled(self.view.connected, egui::Button::new("Check original receipt")).clicked() {
                                action = Some(Action::Lookup(op.clone()));
                            }
                        });
                    }
                });
            }
        });
        if let Some(action) = action {
            self.send(action);
        }
    }
}

impl eframe::App for Desktop {
    fn raw_input_hook(&mut self, ctx: &Context, raw: &mut RawInput) {
        // Even if a confirmation card arrives only in logic() below, an
        // interrupted native frame may not be its first approval frame.
        self.approvals.begin(
            !native_interrupted(raw),
            self.view.card.as_ref().map(|card| &card.claim),
        );
        if matches!(self.gate, Gate::Idle) {
            return;
        }
        let capture = matches!(&self.gate, Gate::Prompt { claim, shown: true }
        if claim.live() && self.view.prompt.as_ref().is_some_and(|ready| {
            ready.claim.same(claim) && Instant::now() < ready.deadline
        }));
        let filtered = self.password.filter(raw, capture);
        // A focus/close revocation still wins while a handed-off credential is
        // queued. Once atomically accepted, keep the in-flight status honest.
        if filtered.abort {
            let unaccepted = match &self.gate {
                Gate::WaitingForCatalog(claim) => claim.revoke(),
                _ => true,
            };
            if unaccepted && !matches!(self.gate, Gate::Retiring) {
                self.retire();
                ctx.request_repaint();
            }
        } else if filtered.rejected {
            self.rejected_input = true;
        } else if self.password.has_value() {
            self.rejected_input = false;
        }
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.update_view();
        if self.approved_send
            && ctx.input(|i| i.viewport().close_requested())
            && self
                .view
                .card
                .as_ref()
                .is_some_and(|card| card.claim.accepted())
        {
            // The raw hook already revoked any not-yet-accepted approval.
            // Do not report an accepted write as canceled on window close.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if matches!(self.gate, Gate::Idle) {
                self.normal(ui);
            } else {
                self.protective_panel(ui);
            }
        });
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        self.retire();
        if let Some(card) = &self.view.card {
            card.claim.revoke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_needs_prior_native_visibility_and_the_same_exact_claim() {
        let old = Claim::new();
        let next = Claim::new();
        let mut frames = ApprovalFrames::default();
        frames.begin(true, None);
        assert!(!frames.show(&old));
        assert!(!frames.show(&old)); // A second egui layout is still the same frame.
        frames.begin(true, Some(&old));
        assert!(frames.show(&old));
        assert_ne!(old.widget_scope(), next.widget_scope());
        assert!(!frames.show(&next));
        assert!(!frames.show(&next));
        frames.begin(true, Some(&next));
        assert!(frames.show(&next));
    }

    #[test]
    fn close_event_wins_over_an_old_or_a_new_card_in_that_frame() {
        let old = Claim::new();
        let next = Claim::new();
        let mut raw = RawInput::default();
        let mut frames = ApprovalFrames::default();
        frames.begin(!native_interrupted(&raw), None);
        assert!(!frames.show(&old));
        raw.viewports
            .get_mut(&raw.viewport_id)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        frames.begin(!native_interrupted(&raw), Some(&old));
        assert!(old.revoked());
        assert!(!frames.show(&old));
        // The owner returns a different card after raw input already ran.
        assert!(!frames.show(&next));
        frames.begin(true, Some(&next));
        assert!(!frames.show(&next));
    }
}
