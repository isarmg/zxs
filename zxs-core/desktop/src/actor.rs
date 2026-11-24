//! One async owner of the current Workspace, its channel and every confirmation.
use std::{
    future::{Future, poll_fn},
    io,
    path::PathBuf,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    task::Poll,
    time::{Duration, Instant},
};

use eframe::egui::Context;
use tokio::sync::{Notify, mpsc, watch};
use zeroize::Zeroizing;
use zxs_core::{
    Result,
    r3::{Body, Branch, Catalog, Container, Detail, DisplayName, Id, Intent, Lookup},
    session::{ExistingBasicLogin, ExistingBasicSession},
    workspace::{Basis, CheckoutChoice, Resolution, Review, Workspace},
};

const PROMPT_LIMIT: Duration = Duration::from_secs(90);
const PENDING_DISPLAY: usize = 32;
const OPEN: u8 = 0;
const REVOKED: u8 = 1;
const ACCEPTED: u8 = 2;

/// A copyable reference to one move-only decision. Revocation is not queued
/// behind a TLS or filesystem await. Acceptance and revocation arbitrate once.
struct Decision {
    phase: AtomicU8,
    revoked: Notify,
}

#[derive(Clone)]
pub struct Claim(Arc<Decision>);

impl Claim {
    pub fn new() -> Self {
        Self(Arc::new(Decision {
            phase: AtomicU8::new(OPEN),
            revoked: Notify::new(),
        }))
    }
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn widget_scope(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    pub fn live(&self) -> bool {
        self.0.phase.load(Ordering::Acquire) == OPEN
    }
    pub fn accepted(&self) -> bool {
        self.0.phase.load(Ordering::Acquire) == ACCEPTED
    }
    pub fn revoked(&self) -> bool {
        self.0.phase.load(Ordering::Acquire) == REVOKED
    }
    pub fn revoke(&self) -> bool {
        if self
            .0
            .phase
            .compare_exchange(OPEN, REVOKED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.0.revoked.notify_one();
            true
        } else {
            false
        }
    }
    fn accept(&self) -> bool {
        self.0
            .phase
            .compare_exchange(OPEN, ACCEPTED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn admit_first_wire(&self, until: Instant) -> Result<()> {
        if Instant::now() >= until || !self.accept() {
            return Err(fail(
                "the original login is no longer approved for the wire",
            ));
        }
        Ok(())
    }

    // Use only with a separately guarded final wire cut. Notification avoids
    // waiting for the H2 timeout while holding an already canceled login.
    async fn unless_revoked<T>(&self, original: impl Future<Output = Result<T>>) -> Result<T> {
        let mut revoke = pin!(self.0.revoked.notified());
        let mut original = pin!(original);
        poll_fn(|cx| {
            if self.revoked() || revoke.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(fail("the original login was canceled")));
            }
            original.as_mut().poll(cx)
        })
        .await
    }
}

pub struct Message(pub u64, pub Action);

pub enum Action {
    Open(PathBuf),
    Disconnect,
    Begin {
        origin: String,
        account: String,
        claim: Claim,
    },
    Finish {
        claim: Claim,
        password: Zeroizing<String>,
    },
    Dismiss,
    Catalog {
        more: bool,
    },
    Detail(Id),
    Choose {
        container: Id,
        branch: Branch,
        node: Id,
        output: PathBuf,
    },
    Review(PathBuf),
    ReviewFirst {
        path: PathBuf,
        name: DisplayName,
    },
    ReviewSend,
    Approve {
        choice: Claim,
        gesture: Claim,
    },
    Lookup(Id),
    PendingPage {
        next: bool,
    },
}

#[derive(Clone)]
pub struct Prompt {
    pub authority: String,
    pub account: String,
    pub claim: Claim,
    pub deadline: Instant,
}

#[derive(Clone, Copy)]
pub enum Effect {
    Checkout,
    Freeze,
    Send,
    FirstUpload,
}

impl Effect {
    pub fn sends(self) -> bool {
        matches!(self, Self::Send | Self::FirstUpload)
    }
}

#[derive(Clone)]
pub enum CardTarget {
    Mapped(Basis),
    First(DisplayName),
}

#[derive(Clone)]
pub struct Card {
    pub claim: Claim,
    pub effect: Effect,
    pub path: String,
    pub target: CardTarget,
    pub body: Body,
    pub intent: Option<Intent>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub epoch: u64,
    pub serial: u64,
    pub path: Option<PathBuf>,
    pub connected: bool,
    pub authority: String,
    pub account: String,
    pub prompt: Option<Prompt>,
    pub rows: Vec<Container>,
    pub after: Option<Id>,
    pub detail: Option<Detail>,
    pub working: Option<PathBuf>,
    pub card: Option<Card>,
    pub pending: Vec<Id>,
    pub pending_start: usize,
    pub pending_more: bool,
    pub fresh_send: bool,
    pub resolution: Option<Resolution>,
    pub notice: &'static str,
}

struct PendingLogin {
    prompt: Prompt,
    channel: ExistingBasicLogin,
}

enum Choice {
    Checkout(CheckoutChoice),
    FirstUpload(Review),
    Freeze(Review),
    Send(Intent),
}

impl Choice {
    fn unconfirmed_message(&self) -> &'static str {
        match self {
            Self::Checkout(_) => {
                "Checkout not confirmed. Check the exact name before trying again; never overwrite it."
            }
            Self::Freeze(_) => {
                "Freeze not confirmed. The working file was not overwritten. Check pending originals."
            }
            Self::Send(_) => {
                "Submit not confirmed. Keep the original operation and check its receipt; do not resend here."
            }
            Self::FirstUpload(_) => {
                "No upload request was sent. The source changed or local staging was refused. Check Pending originals before reviewing again."
            }
        }
    }
}

#[derive(Default)]
struct Owner {
    workspace: Option<Workspace>,
    login: Option<PendingLogin>,
    session: Option<ExistingBasicSession>,
    choice: Option<(Claim, Choice)>,
    fresh: Option<(Intent, String, Basis)>,
    view: Snapshot,
}

pub fn start(ctx: Context) -> Result<(mpsc::Sender<Message>, watch::Receiver<Snapshot>)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (out, input) = mpsc::channel(1);
    let (view, recv) = watch::channel(Snapshot::default());
    std::thread::Builder::new()
        .name("zxs-desktop-worker".into())
        .spawn(move || {
            runtime.block_on(Owner::default().serve(input, view, ctx));
        })?;
    Ok((out, recv))
}

fn fail(message: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::other(message).into()
}

impl Owner {
    async fn serve(
        mut self,
        mut input: mpsc::Receiver<Message>,
        view: watch::Sender<Snapshot>,
        ctx: Context,
    ) {
        loop {
            let message = if let Some(original) = &self.login {
                match tokio::time::timeout_at(original.prompt.deadline.into(), input.recv()).await {
                    Ok(value) => value,
                    Err(_) => {
                        self.drop_login();
                        self.view.notice =
                            "Verified login expired. Verify again before entering a password.";
                        self.publish(&view, &ctx);
                        continue;
                    }
                }
            } else {
                input.recv().await
            };
            let Some(Message(epoch, action)) = message else {
                break;
            };
            if epoch != self.view.epoch {
                self.view.notice = "The selected workspace changed. That action was not used.";
            } else {
                self.apply(action).await;
            }
            self.publish(&view, &ctx);
        }
    }

    fn publish(&mut self, view: &watch::Sender<Snapshot>, ctx: &Context) {
        self.view.serial = self.view.serial.wrapping_add(1);
        view.send_replace(self.view.clone());
        ctx.request_repaint();
    }

    fn drop_login(&mut self) {
        if let Some(login) = self.login.take() {
            login.prompt.claim.revoke();
        }
        self.view.prompt = None;
    }

    fn drop_choice(&mut self) {
        if let Some((claim, _)) = self.choice.take() {
            claim.revoke();
        }
        self.view.card = None;
    }

    fn reset(&mut self, workspace: Option<Workspace>, path: Option<PathBuf>) {
        self.drop_login();
        self.drop_choice();
        self.session = None;
        self.fresh = None;
        self.workspace = workspace;
        self.view = Snapshot {
            epoch: self.view.epoch.wrapping_add(1),
            serial: self.view.serial,
            path,
            ..Snapshot::default()
        };
    }

    fn refresh_pending(&mut self) -> Result<()> {
        let all = self
            .workspace
            .as_ref()
            .ok_or_else(|| fail("no open workspace"))?
            .pending()?;
        let page = pending_page(all.len(), self.view.pending_start);
        self.view.pending_start = page.start;
        self.view.pending_more = page.end < all.len();
        self.view.pending = all[page].to_vec();
        self.view.fresh_send = self.fresh.is_some();
        Ok(())
    }

    fn online(&self) -> Result<(&Workspace, &ExistingBasicSession)> {
        Ok((
            self.workspace
                .as_ref()
                .ok_or_else(|| fail("no open workspace"))?,
            self.session
                .as_ref()
                .ok_or_else(|| fail("no authorized session"))?,
        ))
    }

    fn receive_catalog(&mut self, catalog: Catalog) {
        self.view.rows = catalog.items;
        self.view.after = catalog.next_after;
        self.view.detail = None;
    }

    async fn begin(&mut self, origin: String, account: String, claim: Claim) -> Result<()> {
        self.drop_login();
        if self.session.is_some() || !claim.live() {
            return Err(fail("login not authorized for the current selection"));
        }
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| fail("no open workspace"))?;
        let channel = claim
            .unless_revoked(workspace.begin_existing_basic(&origin, &account))
            .await?;
        if !claim.live() {
            return Err(fail("login choice was revoked while verifying"));
        }
        let prompt = Prompt {
            authority: origin,
            account,
            claim,
            deadline: Instant::now() + PROMPT_LIMIT,
        };
        self.view.prompt = Some(prompt.clone());
        self.login = Some(PendingLogin { prompt, channel });
        Ok(())
    }

    async fn finish(&mut self, claim: &Claim, password: Zeroizing<String>) -> Result<()> {
        let ready = self
            .login
            .as_ref()
            .ok_or_else(|| fail("no verified login choice"))?;
        if !claim.same(&ready.prompt.claim)
            || Instant::now() >= ready.prompt.deadline
            || !claim.live()
        {
            return Err(fail("the exact login choice was revoked or already used"));
        }
        let ready = self
            .login
            .take()
            .ok_or_else(|| fail("lost verified login choice"))?;
        self.view.prompt = None;
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| fail("no open workspace"))?;
        // The fresh typed login remains a future. Cancellation before its
        // first poll drops the password without constructing an auth header.
        // After poll, the shared core checks the cut again before H2 send.
        let until = ready.prompt.deadline;
        let (session, first) = claim
            .unless_revoked(workspace.finish_existing_basic_with_catalog(
                ready.channel,
                password,
                || claim.admit_first_wire(until),
            ))
            .await?;
        self.view.authority = session.principal().origin().to_owned();
        self.view.account = session.principal().account().to_owned();
        self.receive_catalog(first);
        self.view.connected = true;
        self.session = Some(session);
        let _ = self.refresh_pending();
        Ok(())
    }

    fn set_choice(&mut self, choice: Choice, card: Card) {
        self.drop_choice();
        self.choice = Some((card.claim.clone(), choice));
        self.view.card = Some(card);
    }

    async fn choose(&mut self, c: Id, branch: Branch, node: Id, output: PathBuf) -> Result<()> {
        if !self.view.detail.as_ref().is_some_and(|d| {
            d.container_id == c && d.heads.iter().any(|h| h.branch == branch && h.head == node)
        }) {
            return Err(fail("not the displayed original head"));
        }
        let (w, s) = self.online()?;
        let choice = w.choose_checkout(s, &c, &branch, &node, &output).await?;
        let card = Card {
            claim: Claim::new(),
            effect: Effect::Checkout,
            path: choice.output().display().to_string(),
            target: CardTarget::Mapped(choice.basis().clone()),
            body: choice.basis().body.clone(),
            intent: None,
        };
        self.set_choice(Choice::Checkout(choice), card);
        Ok(())
    }

    fn review(&mut self, path: PathBuf) -> Result<&'static str> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| fail("no open workspace"))?;
        if self.fresh.is_some() {
            return Err(fail("finish reviewing the newly staged operation first"));
        }
        let review = workspace.review(&path, None)?;
        if !review.changed() {
            return Ok("Working bytes are unchanged. No operation was created.");
        }
        let card = Card {
            claim: Claim::new(),
            effect: Effect::Freeze,
            path: review.path().to_owned(),
            target: CardTarget::Mapped(
                review
                    .basis()
                    .ok_or_else(|| fail("no original mapping"))?
                    .clone(),
            ),
            body: review.body().clone(),
            intent: None,
        };
        self.set_choice(Choice::Freeze(review), card);
        Ok("Review these full-file bytes. Freezing does not upload.")
    }

    fn review_first(&mut self, path: PathBuf, name: DisplayName) -> Result<()> {
        let (workspace, _) = self.online()?;
        let review = workspace.review(&path, Some(name))?;
        let card = first_card(
            review.path(),
            review
                .name()
                .ok_or_else(|| fail("missing reviewed display name"))?,
            review.body(),
        );
        self.set_choice(Choice::FirstUpload(review), card);
        Ok(())
    }

    fn review_send(&mut self) -> Result<()> {
        let (workspace, _) = self.online()?;
        let (intent, path, basis) = self
            .fresh
            .as_ref()
            .ok_or_else(|| fail("only a newly frozen original can be sent here"))?;
        if workspace.intent(&intent.operation_id)? != *intent {
            return Err(fail("the reviewed original changed"));
        }
        let card = Card {
            claim: Claim::new(),
            effect: Effect::Send,
            path: path.clone(),
            target: CardTarget::Mapped(basis.clone()),
            body: intent.body.clone(),
            intent: Some(intent.clone()),
        };
        self.set_choice(Choice::Send(intent.clone()), card);
        Ok(())
    }

    async fn execute(&mut self, choice: Choice) -> Result<&'static str> {
        let w = self
            .workspace
            .as_mut()
            .ok_or_else(|| fail("no open workspace"))?;
        match choice {
            Choice::Checkout(original) => {
                let path = original.output().to_owned();
                let remote = self
                    .session
                    .as_ref()
                    .ok_or_else(|| fail("no authorized session"))?;
                w.checkout_selected(remote, original, true).await?;
                self.view.working = Some(path);
                Ok("Verified node saved to the exact new ordinary file.")
            }
            Choice::FirstUpload(review) => {
                let remote = self
                    .session
                    .as_ref()
                    .ok_or_else(|| fail("no authorized session"))?;
                // This branch only runs after the original foreground approval.
                // Prepare selects one new container/operation/body and durably
                // retains it before the existing exact submit preflight can POST.
                let intent = match w.prepare(&review, true) {
                    Ok(Some(intent)) => intent,
                    Ok(None) => return Err(fail("no frozen first upload")),
                    Err(error) => {
                        let _ = self.refresh_pending();
                        return Err(error);
                    }
                };
                let result = w
                    .submit_existing(remote, &intent.operation_id, true)
                    .await
                    .unwrap_or(Resolution {
                        operation: intent.operation_id,
                        status: Lookup::Unknown,
                        local_cleanup_pending: false,
                    });
                let confirmed = matches!(result.status, Lookup::Confirmed { .. });
                self.view.resolution = Some(result);
                let _ = self.refresh_pending();
                if confirmed {
                    self.view.working = Some(PathBuf::from(review.path()));
                    Ok(
                        "New remote file confirmed. Select Refresh catalog to see the current listing.",
                    )
                } else {
                    Ok(
                        "Keep the original operation shown below and check its receipt. No automatic resend.",
                    )
                }
            }
            Choice::Freeze(review) => {
                let basis = review
                    .basis()
                    .ok_or_else(|| fail("no original basis"))?
                    .clone();
                let frozen = w.prepare(&review, true)?;
                if let Some(intent) = frozen {
                    self.fresh = Some((intent, review.path().to_owned(), basis));
                    self.view.fresh_send = true;
                    let _ = self.refresh_pending();
                    Ok("Full file frozen locally. Sending requires a separate decision.")
                } else {
                    Ok("Working bytes are unchanged. No operation was created.")
                }
            }
            Choice::Send(original) => {
                let remote = self
                    .session
                    .as_ref()
                    .ok_or_else(|| fail("no authorized session"))?;
                let fresh = self
                    .fresh
                    .take()
                    .ok_or_else(|| fail("the original approval is no longer present"))?;
                self.view.fresh_send = false;
                if fresh.0 != original || w.intent(&original.operation_id)? != original {
                    return Err(fail("the exact original changed before submit"));
                }
                // A failure here never restores the right to submit. Use the
                // retained local original for a subsequent receipt GET only.
                let result = w
                    .submit_existing(remote, &original.operation_id, true)
                    .await?;
                self.view.resolution = Some(result);
                let _ = self.refresh_pending();
                Ok("Original operation checked. See its exact receipt status.")
            }
        }
    }

    async fn approve(&mut self, approved: Claim, gesture: Claim) -> &'static str {
        let Some((claim, _)) = self.choice.as_ref() else {
            return "No current decision to approve.";
        };
        if !claim.same(&approved) || !claim.live() {
            return "That choice is no longer current; nothing was approved.";
        }
        if !gesture.accept() {
            return "Confirmation was interrupted. The same preview is still available.";
        }
        if !claim.accept() {
            return "That choice was canceled before it was accepted.";
        }
        let Some((_, choice)) = self.choice.take() else {
            return "No current decision.";
        };
        self.view.card = None;
        let otherwise = choice.unconfirmed_message();
        self.execute(choice).await.unwrap_or(otherwise)
    }

    async fn lookup(&mut self, op: Id) -> Result<()> {
        let w = self
            .workspace
            .as_mut()
            .ok_or_else(|| fail("no open workspace"))?;
        let remote = self
            .session
            .as_ref()
            .ok_or_else(|| fail("no authorized session"))?;
        let result = w.lookup_existing(remote, &op).await?;
        if self
            .fresh
            .as_ref()
            .is_some_and(|(intent, _, _)| intent.operation_id == op)
            && !matches!(result.status, Lookup::Absent)
        {
            self.fresh = None;
            self.view.fresh_send = false;
        }
        self.view.resolution = Some(result);
        let _ = self.refresh_pending();
        Ok(())
    }

    async fn apply(&mut self, action: Action) {
        if self.login.as_ref().is_some_and(|p| !p.prompt.claim.live()) {
            self.drop_login();
        }
        if self.choice.as_ref().is_some_and(|(claim, _)| !claim.live()) {
            self.drop_choice();
        }
        if (self.login.is_some() && !matches!(&action, Action::Finish { .. } | Action::Dismiss))
            || (self.choice.is_some()
                && !matches!(&action, Action::Approve { .. } | Action::Dismiss))
        {
            self.view.notice = "Resolve or cancel the original visible choice first.";
            return;
        }
        self.view.notice = match action {
            Action::Open(path) => match Workspace::open_local(&path) {
                Ok(workspace) => {
                    self.reset(Some(workspace), Some(path));
                    if self.refresh_pending().is_ok() {
                        "Existing workspace opened. Enter its original origin and account to verify HTTPS."
                    } else {
                        "Workspace opened, but its pending originals could not be read."
                    }
                }
                Err(_) => "That is not an admitted existing workspace. No state was initialized.",
            },
            Action::Disconnect => {
                let workspace = self.workspace.take();
                let path = self.view.path.clone();
                self.reset(workspace, path);
                let _ = self.refresh_pending();
                "Disconnected. No pending operation was sent."
            }
            Action::Begin {
                origin,
                account,
                claim,
            } => {
                if self.begin(origin, account, claim).await.is_ok() {
                    "Original HTTPS/2 destination verified. Enter the password only in the protected prompt."
                } else {
                    "The original HTTPS/2 destination or workspace could not be verified. No password was used."
                }
            }
            Action::Finish { claim, password } => {
                if self.finish(&claim, password).await.is_ok() {
                    "Authorized catalog received on the selected channel."
                } else {
                    self.drop_login();
                    "The selected login did not confirm an authorized catalog. No fallback was tried."
                }
            }
            Action::Dismiss => {
                self.drop_login();
                self.drop_choice();
                "Choice dismissed. No new transfer was authorized."
            }
            Action::Catalog { more } => {
                let result = async {
                    let (workspace, remote) = self.online()?;
                    let after = if more {
                        Some(
                            self.view
                                .after
                                .as_ref()
                                .ok_or_else(|| fail("no next catalog page"))?,
                        )
                    } else {
                        None
                    };
                    workspace.catalog_existing(remote, after).await
                }
                .await;
                if let Ok(catalog) = result {
                    self.drop_choice();
                    self.receive_catalog(catalog);
                    "Verified catalog page received."
                } else {
                    "No verified catalog page was received."
                }
            }
            Action::Detail(c) => {
                let result = async {
                    if !self.view.rows.iter().any(|x| x.container_id == c) {
                        return Err(fail("not the selected row"));
                    }
                    let (workspace, remote) = self.online()?;
                    workspace.detail_existing(remote, &c).await
                }
                .await;
                self.drop_choice();
                if let Ok(detail) = result {
                    self.view.detail = Some(detail);
                    "Select a branch from this verified first detail page."
                } else {
                    self.view.detail = None;
                    "No verified detail was received for that exact row."
                }
            }
            Action::Choose {
                container,
                branch,
                node,
                output,
            } => {
                if self.choose(container, branch, node, output).await.is_ok() {
                    "Confirm the held output directory and the original immutable node."
                } else {
                    "The original head or new ordinary output path could not be confirmed."
                }
            }
            Action::Review(path) => self.review(path).unwrap_or(
                "Select a previously mapped ordinary file without another active local freeze.",
            ),
            Action::ReviewFirst { path, name } => {
                if self.review_first(path, name).is_ok() {
                    "Review this new remote file. Only your final Submit can send it."
                } else {
                    "Choose an unmapped ordinary file and its exact remote name in this authorized workspace."
                }
            }
            Action::ReviewSend => {
                if self.review_send().is_ok() {
                    "Review the exact frozen operation before sending."
                } else {
                    "No current matching frozen original is available to send."
                }
            }
            Action::Approve { choice, gesture } => self.approve(choice, gesture).await,
            Action::PendingPage { next } => {
                let previous = self.view.pending_start;
                self.view.pending_start = if next {
                    self.view.pending_start.saturating_add(PENDING_DISPLAY)
                } else {
                    self.view.pending_start.saturating_sub(PENDING_DISPLAY)
                };
                if self.refresh_pending().is_ok() {
                    "Local pending originals. Select the exact receipt to check."
                } else {
                    self.view.pending_start = previous;
                    "Could not read this workspace's local pending originals."
                }
            }
            Action::Lookup(op) => {
                if self.lookup(op).await.is_ok() {
                    "Original receipt checked; no file body was sent."
                } else {
                    "The original receipt was not confirmed. No file body was sent."
                }
            }
        };
    }
}

fn pending_page(total: usize, requested: usize) -> std::ops::Range<usize> {
    let last = total.saturating_sub(1) / PENDING_DISPLAY * PENDING_DISPLAY;
    let start = requested.min(last);
    start..start.saturating_add(PENDING_DISPLAY).min(total)
}

fn first_card(path: &str, name: &DisplayName, body: &Body) -> Card {
    Card {
        claim: Claim::new(),
        effect: Effect::FirstUpload,
        path: path.to_owned(),
        target: CardTarget::First(name.clone()),
        body: body.clone(),
        intent: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        task::{Context, Waker},
    };

    use super::*;

    #[test]
    fn local_pending_pages_reach_or_clamp_every_original_after_reopen() {
        assert_eq!(pending_page(0, 64), 0..0);
        assert_eq!(pending_page(65, 0), 0..32);
        assert_eq!(pending_page(65, 32), 32..64);
        assert_eq!(pending_page(65, 64), 64..65);
        assert_eq!(pending_page(65, usize::MAX), 64..65);
        assert_eq!(pending_page(31, 64), 0..31);
    }

    #[test]
    fn first_preview_keeps_exact_name_and_body_without_inventing_an_original() {
        let name = DisplayName::parse("试件.bin").unwrap();
        let body = Body {
            length: 167,
            sha256: zxs_core::r3::Digest::parse(&"a".repeat(64)).unwrap(),
        };
        let card = first_card("/ordinary/选中文件", &name, &body);
        assert!(card.effect.sends() && card.claim.live());
        assert_eq!(card.path, "/ordinary/选中文件");
        assert_eq!(card.body, body);
        assert!(matches!(card.target, CardTarget::First(n) if n == name));
        assert!(card.intent.is_none());
    }

    #[test]
    fn prewire_cancel_during_sender_ready_refuses_the_first_auth_cut() {
        for cancel in [false, true] {
            let claim = Claim::new();
            let ready = Cell::new(false);
            let sent = Cell::new(false);
            let until = Instant::now() + PROMPT_LIMIT;
            let mut pending = pin!(claim.unless_revoked(async {
                poll_fn(|_| {
                    if ready.get() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                claim.admit_first_wire(until)?;
                sent.set(true);
                Ok(())
            }));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(pending.as_mut().poll(&mut cx).is_pending());
            if cancel {
                assert!(claim.revoke());
            }
            ready.set(true);
            match pending.as_mut().poll(&mut cx) {
                Poll::Ready(result) => assert_eq!(result.is_err(), cancel),
                Poll::Pending => panic!("simulated ready must return"),
            }
            assert_eq!(sent.get(), !cancel);
            assert!(claim.admit_first_wire(until).is_err());
            assert!(!claim.revoke());
        }
        let expired = Claim::new();
        assert!(expired.admit_first_wire(Instant::now()).is_err());
        assert!(expired.live());
    }

    #[test]
    fn revocation_and_acceptance_are_single_original_decisions() {
        let first = Claim::new();
        let witness = first.clone();
        let other = Claim::new();
        assert!(first.same(&witness));
        assert!(!first.same(&other));
        assert!(witness.revoke());
        assert!(!first.live());
        assert!(!first.accept());
        assert!(other.accept());
        assert!(!other.accept());
        assert!(!other.revoke());
    }
}
