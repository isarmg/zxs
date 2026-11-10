//! Commit the original exact epoch plan to the held member's native decision
//! slot. This is not the later locator/Stop transaction or an Engine writer.
use super::epoch::{ChainEpochSource, SelectedPlan};
use super::*;
use crate::engine::membership_transition::decision::DecisionKind;

const REQUEST: &str = "chain-io-decision-request.json";
const COMPLETE: &str = "chain-io-decision.json";
const TARGET_PLAN: &str = "io-admission-plan.json";
const TARGET_COMPLETE: &str = "io-admission-decision.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "terminal-chain-io-decision")]
    Decided,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Complete {
    format: Format,
    plan_hash: String,
    io_epoch: String,
    publication_hash: String,
    seal_hash: String,
    decision: DecisionReceipt,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainIoDecisionReport {
    pub plan_hash: String,
    pub io_epoch: String,
    pub decision_hash: String,
    pub publication_hash: String,
    pub seal_hash: String,
    pub target_reverified_this_call: bool,
    pub locator_installed: bool,
    pub stop_consumed: bool,
    pub active_writer_admitted: bool,
}
impl Complete {
    /// Expected bytes are a comparison target, not evidence that a vote exists.
    fn expected(plan: &SelectedPlan, source: &ChainEpochSource<'_>) -> Result<Self> {
        plan.verify_source(source)?;
        let hash = plan.object()?.hash;
        Ok(Self {
            format: Format::Decided,
            decision: DecisionReceipt::plan(
                DecisionKind::TerminalChainIoAdmission,
                plan.epoch(),
                &source.request().directory,
                &hash,
                &acceptance(plan, &hash)?,
            )?,
            plan_hash: hash,
            io_epoch: plan.epoch().into(),
            publication_hash: plan.publication_hash().into(),
            seal_hash: plan.seal_hash().into(),
        })
    }
    fn exact(&self, expected: &Self) -> Result<()> {
        if canonical_json(self)? != canonical_json(expected)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "chain I/O decision changed its exact epoch plan, Publication or target vote",
            ));
        }
        Ok(())
    }
    fn report(&self, target: bool) -> TerminalChainIoDecisionReport {
        TerminalChainIoDecisionReport {
            plan_hash: self.plan_hash.clone(),
            io_epoch: self.io_epoch.clone(),
            decision_hash: self.decision.hash().into(),
            publication_hash: self.publication_hash.clone(),
            seal_hash: self.seal_hash.clone(),
            target_reverified_this_call: target,
            locator_installed: false,
            stop_consumed: false,
            active_writer_admitted: false,
        }
    }
}
fn acceptance(plan: &SelectedPlan, hash: &str) -> Result<String> {
    #[derive(Serialize)]
    struct Acceptance<'a> {
        plan: &'a str,
        epoch: &'a str,
        publication: &'a str,
        seal: &'a str,
    }
    Ok(object_hash(
        "terminal-chain-io-acceptance",
        &Acceptance {
            plan: hash,
            epoch: plan.epoch(),
            publication: plan.publication_hash(),
            seal: plan.seal_hash(),
        },
    )?)
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, REQUEST | COMPLETE) || super::generation_log::owns_initial_entry(name)
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    for name in [REQUEST, COMPLETE] {
        if management
            .system_root()
            .optional_metadata(&operation(management).join(name))?
            .is_some()
        {
            return Ok(true);
        }
    }
    super::generation_log::started(management)
}
pub(super) fn require_absent(management: &TerminalManagement) -> Result<()> {
    if started(management)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "chain I/O decision lost its exact original epoch plan",
        ));
    }
    Ok(())
}
fn exact_plan(actual: &SelectedPlan, expected: &SelectedPlan) -> Result<()> {
    if actual.object()?.bytes() != expected.object()?.bytes() {
        return Err(Error::new(
            ErrorCode::LogFork,
            "chain I/O decision retains another original epoch/locator plan",
        ));
    }
    Ok(())
}
/// SYSTEM-only history: the portable receipt is checked, not treated as a fresh
/// target observation. Only explicit decide/repeat opens the actual member.
pub(super) fn observe(
    management: &TerminalManagement,
    plan: &SelectedPlan,
    source: &ChainEpochSource<'_>,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    let root = management.system_root();
    let Some(saved): Option<SelectedPlan> = load(root, &operation(management).join(REQUEST))?
    else {
        require_absent(management)?;
        return Ok(());
    };
    exact_plan(&saved, plan)?;
    status.phase = TerminalChainReadmissionPhase::IoDecisionPending;
    if let Some(complete) = load::<Complete>(root, &operation(management).join(COMPLETE))? {
        complete.exact(&Complete::expected(plan, source)?)?;
        status.phase = TerminalChainReadmissionPhase::IoDecisionPreserved;
        status.io_decision = Some(complete.report(false));
    }
    Ok(())
}

struct Presence {
    request: bool,
    source_complete: bool,
    target_plan: bool,
    target_complete: bool,
}
struct Owner<'a, 'm> {
    management: &'m TerminalManagement,
    reader: &'a TerminalChainPublishedCurrent<'m>,
    source: ChainEpochSource<'a>,
    plan: SelectedPlan,
    expected: Complete,
    target: DiskRoot,
}
impl<'a, 'm> Owner<'a, 'm> {
    fn open(
        management: &'m TerminalManagement,
        reader: &'a TerminalChainPublishedCurrent<'m>,
    ) -> Result<Self> {
        let source = reader.epoch_source()?;
        let plan = epoch::selected_plan(management, &source)?;
        let expected = Complete::expected(&plan, &source)?;
        // Encoding is bounded before a source reservation or target mutation.
        PreparedMetadataObject::new("terminal-chain-io-decision", &expected, MAX_METADATA)?;
        let held = reader.held_target();
        let selection = reader.selection();
        let target = held.root.private_child(&pending::member_directory(
            selection.proposal.target_disk(),
            &selection.hash()?,
        ))?;
        Ok(Self {
            management,
            reader,
            source,
            plan,
            expected,
            target,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.reader.recheck_publication()?;
        let actual = epoch::selected_plan(self.management, &self.source)?;
        exact_plan(&actual, &self.plan)?;
        let held = self.reader.held_target();
        if held.root.directory_identity(self.target.path())?
            != self.target.directory_identity(self.target.path())?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "chain I/O decision parent was replaced",
            ));
        }
        self.reader.recheck_held_target()
    }
    fn observe_files(&self) -> Result<Presence> {
        let root = self.management.system_root();
        let request: Option<SelectedPlan> = load(root, &operation(self.management).join(REQUEST))?;
        let source_complete: Option<Complete> =
            load(root, &operation(self.management).join(COMPLETE))?;
        let target_plan: Option<SelectedPlan> =
            load(&self.target, &self.target.path().join(TARGET_PLAN))?;
        let target_complete: Option<Complete> =
            load(&self.target, &self.target.path().join(TARGET_COMPLETE))?;
        let slot = DecisionStore::terminal_chain_io_slot_present(
            &self.reader.held_target().root,
            self.plan.epoch(),
        )?;
        if (request.is_none()
            && (source_complete.is_some()
                || target_plan.is_some()
                || target_complete.is_some()
                || slot))
            || (target_plan.is_none()
                && (slot || target_complete.is_some() || source_complete.is_some()))
            || (source_complete.is_some() && target_complete.is_none())
            || (target_complete.is_some() && !slot)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "chain I/O decision lost earlier durable evidence; do not rebuild or re-vote",
            ));
        }
        for plan in [&request, &target_plan].into_iter().flatten() {
            exact_plan(plan, &self.plan)?;
        }
        for complete in [&source_complete, &target_complete].into_iter().flatten() {
            complete.exact(&self.expected)?;
        }
        Ok(Presence {
            request: request.is_some(),
            source_complete: source_complete.is_some(),
            target_plan: target_plan.is_some(),
            target_complete: target_complete.is_some(),
        })
    }
    fn reserve_capacity(&self, presence: &Presence) -> Result<()> {
        let plan = self.plan.object()?.bytes().len() as u64;
        let receipt = canonical_json(&self.expected)?.len() as u64;
        let source = if presence.request { 0 } else { plan }
            + if presence.source_complete { 0 } else { receipt };
        let target = if presence.target_plan { 0 } else { plan }
            + if presence.target_complete {
                0
            } else {
                receipt + 16 * 1024
            };
        for (root, bytes) in [
            (self.management.system_root(), source),
            (&self.reader.held_target().root, target),
        ] {
            if bytes != 0 && root.available()? < bytes * 2 + 1024 * 1024 {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "chain I/O decision requires independent source and target capacity",
                ));
            }
        }
        Ok(())
    }
    fn preserve_plan(&self, root: &DiskRoot, path: &Path) -> Result<()> {
        root.write_json(path, &self.plan, false)?;
        let saved: SelectedPlan = decode_strict(&root.read_bytes(path, MAX_METADATA)?)?;
        exact_plan(&saved, &self.plan)
    }
    fn read_target(&self, store: &DecisionStore) -> Result<Complete> {
        let saved: SelectedPlan = decode_strict(
            &self
                .target
                .read_bytes(&self.target.path().join(TARGET_PLAN), MAX_METADATA)?,
        )?;
        exact_plan(&saved, &self.plan)?;
        let complete: Complete = decode_strict(
            &self
                .target
                .read_bytes(&self.target.path().join(TARGET_COMPLETE), MAX_METADATA)?,
        )?;
        complete.exact(&self.expected)?;
        store.verify_receipt(&complete.decision, true)?;
        self.reader.recheck_publication()?;
        Ok(complete)
    }
    fn decide(&self) -> Result<TerminalChainIoDecisionReport> {
        self.recheck()?;
        let presence = self.observe_files()?;
        self.reserve_capacity(&presence)?;
        let root = self.management.system_root();
        // The original SYSTEM plan becomes an explicit decision request before
        // any target write. A lost response never permits a different epoch.
        self.preserve_plan(root, &operation(self.management).join(REQUEST))?;
        self.preserve_plan(&self.target, &self.target.path().join(TARGET_PLAN))?;
        self.recheck()?;
        let held = self.reader.held_target();
        let store = DecisionStore::open(
            &held.root,
            &held.identity,
            self.plan.epoch(),
            !presence.target_complete,
        )?;
        let actual = store.receipt(
            DecisionKind::TerminalChainIoAdmission,
            &self.expected.plan_hash,
            &acceptance(&self.plan, &self.expected.plan_hash)?,
            !presence.target_complete,
            #[cfg(feature = "fault-injection")]
            None,
        )?;
        if actual != self.expected.decision {
            return Err(Error::new(
                ErrorCode::LogFork,
                "original target selected another I/O decision",
            ));
        }
        let mut complete = self.expected.clone();
        complete.decision = actual;
        self.recheck()?;
        if !presence.target_complete {
            self.target
                .write_json(&self.target.path().join(TARGET_COMPLETE), &complete, false)?;
        }
        let complete = self.read_target(&store)?;
        self.recheck()?;
        // A cold target-complete/source-incomplete retry reaches only this
        // no-overwrite receipt completion after re-reading the actual target.
        if !presence.source_complete {
            root.write_json(&operation(self.management).join(COMPLETE), &complete, false)?;
        }
        let saved: Complete = decode_strict(
            &root.read_bytes(&operation(self.management).join(COMPLETE), MAX_METADATA)?,
        )?;
        saved.exact(&complete)?;
        self.read_target(&store)?;
        self.recheck()?;
        Ok(saved.report(true))
    }
}
impl TerminalManagement {
    /// Choose the original epoch on the actual retained member. No locator,
    /// Stop, disk seal, active Current, journal writer or service is changed.
    pub fn decide_chain_io_admission(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        let reader =
            self.open_chain_published_current(principal, operation_id, proposal_digest, directory)?;
        let report = Owner::open(self, &reader)?.decide()?;
        drop(reader);
        let mut status = self
            .chain_readmission_status()?
            .expect("invariant: original source decision remains owned");
        status.io_decision = Some(report);
        Ok(status)
    }
}

/// A new consumer must reread the actual completed target slot, not accept the
/// serializable SYSTEM report as a permission. This function never fills holes.
pub(super) fn verify_existing<'m>(
    management: &'m TerminalManagement,
    reader: &TerminalChainPublishedCurrent<'m>,
) -> Result<TerminalChainIoDecisionReport> {
    let owner = Owner::open(management, reader)?;
    owner.recheck()?;
    let presence = owner.observe_files()?;
    if !presence.request
        || !presence.source_complete
        || !presence.target_plan
        || !presence.target_complete
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "complete the original two-root I/O decision before generation installation",
        ));
    }
    let held = reader.held_target();
    let store = DecisionStore::open(&held.root, &held.identity, owner.plan.epoch(), false)?;
    let complete = owner.read_target(&store)?;
    let source: Complete = decode_strict(
        &management
            .system_root()
            .read_bytes(&operation(management).join(COMPLETE), MAX_METADATA)?,
    )?;
    source.exact(&complete)?;
    owner.recheck()?;
    Ok(complete.report(true))
}
