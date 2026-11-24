//! One window with explicit typed actions; no networking or SQLite on the UI thread.
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use eframe::egui::{self, Context, Event, Key, PointerButton, RawInput, Response, Ui};
use tokio::sync::{mpsc, watch};
use zxs_core::{
    Result,
    r3::{Branch, DisplayName, Lookup},
};

use crate::{
    actor::{self, Action, Card, CardTarget, Claim, Effect, Message, Snapshot},
    clipboard::{Queue as Clipboard, Target as PasteTarget},
    picker::{self, Field},
    secret::{NativeFrame, Password, clipboard_chord},
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
    fn begin(&mut self, frame: NativeFrame, existing: Option<&Claim>, gesture: Option<&Claim>) {
        self.native = self.native.wrapping_add(1);
        self.admitted = frame.active();
        if !self.admitted {
            if let Some(gesture) = gesture {
                gesture.revoke();
            }
            self.seen = None;
        }
        if frame.explicit_end()
            && let Some(existing) = existing
        {
            existing.revoke();
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
    gesture: Option<Claim>,
    approvals: ApprovalFrames,
    chooser: Option<picker::Selection>,
    local_notice: Option<&'static str>,
    form_enter: Option<egui::Id>,
    ordinary_paste: Option<egui::Id>,
    clipboard: Clipboard,
    password_paste: bool,
    password_enter: bool,
    gate: Gate,
    password: Password,
    rejected_input: bool,
    clipboard_unavailable: bool,
    state_path: String,
    origin: String,
    account: String,
    destination: String,
    branch: Option<Branch>,
    working: String,
    first_path: String,
    first_name: String,
    first_name_automatic: bool,
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
            gesture: None,
            approvals: ApprovalFrames::default(),
            chooser: None,
            local_notice: None,
            form_enter: None,
            ordinary_paste: None,
            clipboard: Clipboard::default(),
            password_paste: false,
            password_enter: false,
            gate: Gate::Idle,
            password: Password::default(),
            rejected_input: false,
            clipboard_unavailable: false,
            state_path: String::new(),
            origin: String::new(),
            account: String::new(),
            destination: String::new(),
            branch: None,
            working: String::new(),
            first_path: String::new(),
            first_name: String::new(),
            first_name_automatic: false,
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
        self.clipboard_unavailable = false;
        self.password_enter = false;
        self.password_paste = false;
        self.gate = Gate::Retiring;
    }

    fn choose_path(&mut self, ctx: &Context, field: Field) {
        match picker::Selection::ask(ctx, self.view.epoch, field) {
            Ok(chooser) => {
                self.chooser = Some(chooser);
                self.local_notice = None;
            }
            Err(_) => {
                self.local_notice =
                    Some("Could not open the native chooser. Enter the absolute path instead.")
            }
        }
    }

    fn poll_chooser(&mut self) {
        let Some(chooser) = &self.chooser else {
            return;
        };
        let Some(path) = chooser.poll() else {
            return;
        };
        let epoch = chooser.epoch;
        let field = chooser.field;
        self.chooser = None;
        if epoch != self.view.epoch || !matches!(self.gate, Gate::Idle) || self.view.card.is_some()
        {
            return;
        }
        let Some(path) = path else {
            self.local_notice =
                Some("No path selected. If the chooser is unavailable, enter the path directly.");
            return;
        };
        let Some(path) = path.to_str().filter(|p| Path::new(p).is_absolute()) else {
            self.local_notice = Some("The chooser did not return a local absolute UTF-8 path.");
            return;
        };
        match field {
            Field::Workspace => self.state_path = path.to_owned(),
            Field::CheckoutFolder => {
                self.destination = format!("{}/", path.trim_end_matches('/'));
                self.local_notice = Some(
                    "Append the exact new filename. Checkout never replaces an existing file.",
                );
            }
            Field::WorkingFile => self.working = path.to_owned(),
            Field::FirstFile => {
                self.first_path = path.to_owned();
                offer_first_name(&mut self.first_name, &mut self.first_name_automatic, path);
            }
        }
    }

    fn update_view(&mut self) {
        match self.updates.has_changed() {
            Ok(false) => return,
            Ok(true) => {}
            Err(_) => {
                self.waiting = false;
                self.approved_send = false;
                if let Some(gesture) = self.gesture.take() {
                    gesture.revoke();
                }
                self.chooser = None;
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
            self.chooser = None;
            self.local_notice = None;
            self.password.clear();
            if let Some(gesture) = self.gesture.take() {
                gesture.revoke();
            }
            if !matches!(self.gate, Gate::Idle) {
                self.retire();
            }
            self.destination.clear();
            self.working.clear();
            self.first_path.clear();
            self.first_name.clear();
            self.first_name_automatic = false;
        }
        if next.serial != self.view.serial {
            self.waiting = false;
            self.approved_send = false;
            self.gesture = None;
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
                    .add_enabled(self.approvals.admitted, egui::Button::new("Verify HTTPS/2"))
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
                ui.label(
                    "Type or deliberately paste here. Enter signs in after composition finishes.",
                );
                let field = ui.label(if self.password.has_value() {
                    "Password entered (hidden)"
                } else {
                    "No password entered"
                });
                if self.approvals.admitted {
                    ui.output_mut(|output| {
                        output.ime = Some(egui::output::IMEOutput {
                            purpose: egui::IMEPurpose::Password,
                            rect: field.rect,
                            cursor_rect: egui::Rect::from_min_size(
                                field.rect.left_bottom(),
                                egui::vec2(1.0, field.rect.height()),
                            ),
                            should_interrupt_composition: false,
                        });
                    });
                }
                if self.rejected_input {
                    ui.label("Input was discarded: the password is too long or contains control characters.");
                } else if self.clipboard_unavailable {
                    ui.label("Could not read clipboard text. Type the password or use a supported desktop clipboard.");
                }
                let target = PasteTarget::Protected(claim.clone());
                if was_shown && self.approvals.admitted && std::mem::take(&mut self.password_paste)
                {
                    self.clipboard.request(ui.ctx(), target.clone());
                }
                let enter = std::mem::take(&mut self.password_enter);
                let ready = self.approvals.admitted
                    && !self.clipboard.waiting_for(&target)
                    && was_shown
                    && self.password.ready_to_submit()
                    && !self.waiting;
                let sign_in =
                    ui.add_enabled(ready, egui::Button::new("Sign in to this exact workspace"));
                if ui.button("Cancel").clicked() {
                    self.retire();
                } else if ready
                    && (enter || sign_in.clicked_by(PointerButton::Primary))
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
            Effect::FirstUpload => "Submit as a new remote file",
            Effect::FirstResumable => "Submit a new file with resumable upload",
            Effect::Resume => "Continue this exact frozen upload",
        });
        ui.label(format!("File: {}", card.path));
        match &card.target {
            CardTarget::Mapped(basis) => {
                ui.label(format!("Container: {}", basis.container));
                ui.label(format!("Branch: {}", basis.branch));
                ui.label(format!("Approved head: {}", basis.node));
                ui.label(format!("Container revision: {}", basis.seen_sequence));
            }
            CardTarget::First(name) => {
                ui.label(format!("New remote name: {name}"));
                ui.label(format!(
                    "Destination account: {} · {}",
                    self.view.authority, self.view.account
                ));
                if card.intent.is_none() {
                    ui.label("Submit will stage this exact full file and use one original operation. The ordinary file is not replaced.");
                }
            }
        }
        ui.label(format!("Full body: {} bytes", card.body.length));
        ui.label(format!("SHA-256: {}", card.body.sha256));
        if let Some(intent) = &card.intent {
            ui.label(format!("Operation: {}", intent.operation_id));
            ui.label(format!("New node: {}", intent.node_id));
            ui.label("Only authoritative absence of this exact receipt permits sending these frozen bytes.");
        }
        if matches!(card.effect, Effect::FirstResumable | Effect::Resume) {
            ui.label("Checks the original receipt first, then uploads only the remaining chunks and publishes. No full-file fallback.");
            if let Some(done) = card.claim.confirmed_bytes() {
                ui.label(format!(
                    "Server-confirmed bytes: {done}/{} (not a publication receipt)",
                    card.body.length
                ));
            }
            if self.waiting {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(300));
            }
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
                egui::Button::new(match card.effect {
                    Effect::FirstUpload => "Submit new file",
                    Effect::FirstResumable => "Submit new file with resume",
                    Effect::Resume => "Continue and publish this original",
                    _ => "Confirm this exact choice",
                }),
            )
            .clicked_by(PointerButton::Primary)
        {
            let gesture = Claim::new();
            if self.send(Action::Approve {
                choice: card.claim.clone(),
                gesture: gesture.clone(),
            }) {
                self.gesture = Some(gesture);
                self.approved_send = card.effect.sends();
            }
        }
        if self.approved_send && self.gesture.as_ref().is_some_and(Claim::accepted) {
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
        let mut pick = None;
        let enter = self.form_enter.take();
        if let Some(notice) = self.local_notice {
            ui.label(notice);
        }
        if self.chooser.is_some() {
            ui.label("Waiting for the native file chooser. No transfer was started.");
        }
        ui.add_enabled_ui(!self.waiting && self.chooser.is_none() && self.approvals.admitted, |ui| {
            if let Some(path) = &self.view.path {
                ui.label(format!("Original private workspace: {}", path.display()));
            }
            ui.horizontal(|ui| {
                ui.label("Existing state directory:");
                let edit = ui.text_edit_singleline(&mut self.state_path);
                if ui.button("Choose folder").clicked() {
                    pick = Some(Field::Workspace);
                }
                if ui.button("Open existing").clicked() || field_enter(&edit, enter) {
                    if Path::new(&self.state_path).is_absolute() {
                        action = Some(Action::Open(PathBuf::from(&self.state_path)));
                    } else {
                        self.local_notice = Some("Select the existing private workspace using an absolute folder path.");
                    }
                }
            });
            if !self.view.connected {
                ui.label("This app opens existing enrolled workspaces. It does not create a new one.");
                ui.horizontal(|ui| {
                    ui.label("Original HTTPS origin");
                    let origin = ui.text_edit_singleline(&mut self.origin);
                    ui.label("Account");
                    let account = ui.text_edit_singleline(&mut self.account);
                    if ui.button("Review connection").clicked()
                        || field_enter(&origin, enter) || field_enter(&account, enter)
                    {
                        if self.view.path.is_none() {
                            self.local_notice = Some("Open the existing private workspace first.");
                            return;
                        }
                        if !self.origin.starts_with("https://") || self.account.is_empty() {
                            self.local_notice = Some("Enter the original HTTPS origin and account.");
                            return;
                        }
                        self.local_notice = None;
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
                    if ui.button("Lock workspace").clicked() {
                        action = Some(Action::Disconnect);
                    }
                });
                ui.separator();
                ui.heading("New remote file");
                let mut first_review = None;
                ui.horizontal(|ui| {
                    ui.label("Ordinary file:");
                    let file = ui.text_edit_singleline(&mut self.first_path);
                    if ui.button("Choose file…").clicked() {
                        pick = Some(Field::FirstFile);
                    }
                    ui.label("Remote name:");
                    if file.changed() {
                        offer_first_name(&mut self.first_name, &mut self.first_name_automatic, &self.first_path);
                    }
                    let name = ui.text_edit_singleline(&mut self.first_name);
                    if name.changed() {
                        self.first_name_automatic = false;
                    }
                    if ui.button("Review first upload").clicked() || field_enter(&file, enter) || field_enter(&name, enter) {
                        first_review = Some(false);
                    }
                });
                if ui.button("Review resumable upload").clicked() {
                    first_review = Some(true);
                }
                if let Some(resumable) = first_review {
                    if !absolute_file(&self.first_path) {
                        self.local_notice = Some("Choose an ordinary file using an absolute path.");
                    } else if let Ok(name) = DisplayName::parse(&self.first_name) {
                        action = Some(Action::ReviewFirst { path: self.first_path.as_str().into(), name, resumable });
                    } else {
                        self.local_notice = Some("Enter one exact remote filename, without a directory or trailing dot or space.");
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Refresh catalog").clicked() { action = Some(Action::Catalog { more: false }); }
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
                        let edit = ui.text_edit_singleline(&mut self.destination);
                        if ui.button("Choose output folder").clicked() {
                            pick = Some(Field::CheckoutFolder);
                        }
                        let head = self.branch.as_ref().and_then(|b| detail.heads.iter().find(|h| &h.branch == b));
                        if ui.button("Review checkout").clicked() || field_enter(&edit, enter) {
                            if !absolute_file(&self.destination) {
                                self.local_notice = Some("Choose an absolute output path with an exact new filename.");
                            } else if let Some(head) = head {
                                action = Some(Action::Choose {
                                    container: detail.container_id.clone(), branch: head.branch.clone(),
                                    node: head.head.clone(), output: self.destination.as_str().into(),
                                });
                            } else {
                                self.local_notice = Some("Select the branch whose exact head you want to check out.");
                            }
                        }
                    });
                }
            }
            if self.view.path.is_some() {
                ui.separator();
                ui.label("Edit the ordinary path in your preferred application. Editor saves never submit.");
                ui.horizontal(|ui| {
                    ui.label("Mapped working file:");
                    let edit = ui.text_edit_singleline(&mut self.working);
                    if ui.button("Choose file").clicked() {
                        pick = Some(Field::WorkingFile);
                    }
                    if !self.view.fresh_send && (ui.button("Review full change").clicked() || field_enter(&edit, enter)) {
                        if absolute_file(&self.working) {
                            action = Some(Action::Review(self.working.as_str().into()));
                        } else {
                            self.local_notice = Some("Choose a mapped ordinary file using an absolute path.");
                        }
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
                    if self.view.connected && !matches!(result.status, Lookup::Confirmed { .. })
                        && !self.view.pending.contains(&result.operation)
                        && ui.button("Check this original receipt").clicked()
                    {
                        action = Some(Action::Lookup(result.operation.clone()));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.view.pending_start > 0, egui::Button::new("Previous local page")).clicked() {
                        action = Some(Action::PendingPage { next: false });
                    }
                    if ui.add_enabled(self.view.pending_more, egui::Button::new("Next local page")).clicked() {
                        action = Some(Action::PendingPage { next: true });
                    }
                });
                egui::ScrollArea::vertical().id_salt("pending").max_height(120.0).show(ui, |ui| {
                    for op in &self.view.pending {
                        ui.horizontal(|ui| {
                            ui.label(op.as_str());
                            if ui.add_enabled(self.view.connected, egui::Button::new("Check original receipt")).clicked() {
                                action = Some(Action::Lookup(op.clone()));
                            }
                            if self.view.connected && self.view.resume_candidates.contains(op)
                                && ui.button("Review resumable upload").clicked() {
                                action = Some(Action::ReviewResume(op.clone()));
                            }
                        });
                    }
                });
            }
        });
        if let Some(field) = pick {
            self.choose_path(ui.ctx(), field);
        } else if let Some(action) = action {
            self.local_notice = None;
            self.send(action);
        } else if let Some(edit) = self.ordinary_paste.take()
            && matches!(self.gate, Gate::Idle)
            && self.approvals.admitted
            && ui.ctx().memory(|memory| memory.focused()) == Some(edit)
        {
            self.clipboard
                .request(ui.ctx(), PasteTarget::Ordinary(edit));
        }
    }
}

fn offer_first_name(name: &mut String, automatic: &mut bool, path: &str) {
    if *automatic || name.is_empty() {
        *name = Path::new(path)
            .file_name()
            .and_then(|leaf| leaf.to_str())
            .filter(|leaf| DisplayName::parse(leaf).is_ok())
            .unwrap_or_default()
            .to_owned();
        *automatic = !name.is_empty();
    }
}

fn absolute_file(path: &str) -> bool {
    Path::new(path).is_absolute() && !path.ends_with('/') && Path::new(path).file_name().is_some()
}

fn field_enter(edit: &Response, enter: Option<egui::Id>) -> bool {
    enter == Some(edit.id) && edit.lost_focus()
}

fn ordinary_clipboard(focused: Option<egui::Id>, raw: &mut RawInput) -> Option<egui::Id> {
    // Native shortcuts are disabled globally to avoid a pre-hook clipboard
    // text copy in the protected lane. Only normal widgets use the native
    // integration's copy/cut events and requested paste.
    let focused = focused?;
    let mut paste = false;
    raw.events.retain_mut(|event| {
        if let Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
            && clipboard_chord(*modifiers)
        {
            match key {
                Key::C => *event = Event::Copy,
                Key::X => *event = Event::Cut,
                Key::V => {
                    paste = true;
                    return false;
                }
                _ => {}
            }
        }
        true
    });
    paste.then_some(focused)
}

impl eframe::App for Desktop {
    fn raw_input_hook(&mut self, ctx: &Context, raw: &mut RawInput) {
        let frame = NativeFrame::from_raw(raw);
        self.approvals.begin(
            frame,
            self.view.card.as_ref().map(|card| &card.claim),
            self.gesture.as_ref(),
        );
        self.password_enter = false;
        self.password_paste = false;
        self.form_enter = None;
        self.ordinary_paste = None;
        if frame.explicit_end() {
            self.chooser = None;
        }

        let ordinary = matches!(self.gate, Gate::Idle)
            && frame.active()
            && self.view.card.is_none()
            && self.chooser.is_none();
        let focused = ordinary
            .then(|| ctx.memory(|m| m.focused()))
            .flatten()
            .filter(|id| egui::TextEdit::load_state(ctx, *id).is_some());
        let prompt = match &self.gate {
            Gate::Prompt { claim, shown: true }
                if frame.active()
                    && claim.live()
                    && self.view.prompt.as_ref().is_some_and(|ready| {
                        ready.claim.same(claim) && Instant::now() < ready.deadline
                    }) =>
            {
                Some(PasteTarget::Protected(claim.clone()))
            }
            _ => None,
        };
        let target = focused
            .map(PasteTarget::Ordinary)
            .or_else(|| prompt.clone());
        let paste_waiting = prompt
            .as_ref()
            .is_some_and(|t| self.clipboard.waiting_for(t));
        let no_clipboard = self.clipboard.receive(raw, target.as_ref(), Instant::now());

        if ordinary {
            if no_clipboard {
                self.local_notice = Some("No clipboard text is available to this field.");
            }
            if raw.events.iter().any(|e| {
                matches!(e, Event::Key {
                key: Key::Enter, pressed: true, modifiers, ..
            } if *modifiers == egui::Modifiers::NONE)
            }) && !raw
                .events
                .iter()
                .any(|e| matches!(e, Event::Ime(_) | Event::PointerButton { .. }))
            {
                self.form_enter = focused;
            }
            self.ordinary_paste = ordinary_clipboard(focused, raw);
            return;
        }
        let filtered = self
            .password
            .filter(raw, prompt.is_some(), frame, paste_waiting);
        self.password_enter = filtered.submit;
        self.password_paste = filtered.paste_requested;
        if frame.explicit_end() && !matches!(self.gate, Gate::Idle | Gate::Retiring) {
            self.retire();
            ctx.request_repaint();
        } else if filtered.rejected || no_clipboard {
            self.rejected_input = filtered.rejected;
            self.clipboard_unavailable = no_clipboard;
        } else if self.password.has_value() {
            self.rejected_input = false;
            self.clipboard_unavailable = false;
        }
        // Once a foreground sign-in was explicitly handed off, transient
        // blur is not cancellation. The same held H2-ready guard still
        // honors explicit cancel, close, change and expiry.
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.update_view();
        self.poll_chooser();
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
        self.clipboard.painted(ui.ctx(), self.approvals.native);
        egui::CentralPanel::default().show(ui, |ui| {
            if matches!(self.gate, Gate::Idle) {
                ui.push_id(("workspace", self.view.epoch), |ui| self.normal(ui));
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
        if let Some(gesture) = &self.gesture {
            gesture.revoke();
        }
        if let Some(card) = &self.view.card {
            card.claim.revoke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_file_replaces_only_an_automatically_suggested_name() {
        let mut name = String::new();
        let mut automatic = false;
        offer_first_name(&mut name, &mut automatic, "/ordinary/初次.txt");
        assert_eq!((&name[..], automatic), ("初次.txt", true));
        offer_first_name(&mut name, &mut automatic, "/ordinary/第二份.txt");
        assert_eq!((&name[..], automatic), ("第二份.txt", true));
        name = "My visible name".to_owned();
        automatic = false;
        offer_first_name(&mut name, &mut automatic, "/ordinary/third.txt");
        assert_eq!((&name[..], automatic), ("My visible name", false));
    }

    #[test]
    fn a_card_needs_prior_native_visibility_and_the_same_exact_claim() {
        let old = Claim::new();
        let next = Claim::new();
        let mut frames = ApprovalFrames::default();
        frames.begin(NativeFrame::Active, None, None);
        assert!(!frames.show(&old));
        assert!(!frames.show(&old));
        frames.begin(NativeFrame::Active, Some(&old), None);
        assert!(frames.show(&old));
        assert_ne!(old.widget_scope(), next.widget_scope());
        assert!(!frames.show(&next));
        assert!(!frames.show(&next));
        frames.begin(NativeFrame::Active, Some(&next), None);
        assert!(frames.show(&next));
    }

    #[test]
    fn close_event_wins_over_an_old_or_a_new_card_in_that_frame() {
        let old = Claim::new();
        let next = Claim::new();
        let mut raw = RawInput::default();
        let mut frames = ApprovalFrames::default();
        frames.begin(NativeFrame::from_raw(&raw), None, None);
        assert!(!frames.show(&old));
        raw.viewports
            .get_mut(&raw.viewport_id)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        frames.begin(NativeFrame::from_raw(&raw), Some(&old), None);
        assert!(old.revoked());
        assert!(!frames.show(&old));
        assert!(!frames.show(&next));
        frames.begin(NativeFrame::Active, Some(&next), None);
        assert!(!frames.show(&next));
    }

    #[test]
    fn focus_loss_revokes_only_the_gesture_and_requires_a_new_foreground_frame() {
        let card = Claim::new();
        let gesture = Claim::new();
        let mut frames = ApprovalFrames::default();
        frames.begin(NativeFrame::Active, Some(&card), None);
        assert!(!frames.show(&card));
        frames.begin(NativeFrame::Inactive, Some(&card), Some(&gesture));
        assert!(gesture.revoked() && card.live());
        assert!(!frames.show(&card));
        frames.begin(NativeFrame::Active, Some(&card), None);
        assert!(!frames.show(&card));
        frames.begin(NativeFrame::Active, Some(&card), None);
        assert!(frames.show(&card) && gesture.revoked());
    }
}
