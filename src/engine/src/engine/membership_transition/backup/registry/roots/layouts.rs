//! One immutable whole-target namespace feedback, retained by the same actual
//! original fence holders before the common decision can select it.
use super::*;
use crate::container_storage::{BackupLayoutPlan, BackupTargetLayout};
use crate::history_package::AdmittedState;

const MAX_LAYOUT: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Feedback {
    source_fence: SourceFenceHash,
    operation_id: OperationId,
    targets: BTreeMap<DiskId, BackupTargetLayout>,
}

pub(in crate::engine) struct PreparedBackupLayouts {
    feedback: Feedback,
}
impl PreparedBackupLayouts {
    pub(in crate::engine) fn prepare(
        roots: &PreparedBackupRoots, choices: &BackupChoiceContext,
        targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<Self> {
        choices.recheck_root_sources(stop)?;
        let mut saved = None;
        for source in choices.sources.values() {
            source.recheck(stop)?;
            if let Some(value) = read(source, choices.intent.operation_id)? {
                require_source(&value, &choices.transition_hash, choices.intent.operation_id)?;
                merge(&mut saved, value)?;
            }
        }
        let feedback = match saved {
            Some(value) => value,
            None => {
                // All target ids, complete metadata and name/space budgets are
                // admitted before the first target preparation is created.
                let mut plans: Vec<BackupLayoutPlan> = Vec::with_capacity(targets.len());
                let mut bytes = 1024usize;
                for index in 0..targets.len() {
                    let plan = roots.with_layout_preparation(choices, targets, index,
                        original, stop, |source| BackupTargetLayout::plan(source, stop))?;
                    bytes = bytes.checked_add(plan.bytes()?).and_then(|v| v.checked_add(128))
                        .ok_or(Failure::BackupRootBudget)?;
                    if bytes > MAX_LAYOUT {
                        return Err(Failure::BackupRootBudget.into());
                    }
                    plans.push(plan);
                }
                for source in choices.sources.values() {
                    if source.root.available()? < RESERVE + MAX_LAYOUT as u64 {
                        return Err(Failure::BackupRootSpace.into());
                    }
                }
                let mut layouts = BTreeMap::new();
                for (index, plan) in plans.into_iter().enumerate() {
                    let value = roots.with_layout_preparation(choices, targets, index,
                        original, stop, |source| BackupTargetLayout::prepare(source, plan, stop))?;
                    let id = choices.root_evidence()?.require_root_target(index,
                        targets[index].layout_root()?)?;
                    layouts.insert(id, value);
                }
                Feedback { source_fence: SourceFenceHash::from_transition(&choices.transition_hash),
                    operation_id: choices.intent.operation_id, targets: layouts }
            }
        };
        let product = Self { feedback };
        product.observe_live(roots, choices, targets, original, stop)?;
        let bytes = encoded(&product.feedback)?;
        for source in choices.sources.values() {
            source.recheck(stop)?;
            install_preparation_bytes(&source.root,
                &path(&source.root, product.feedback.operation_id), &bytes)?;
        }
        let confirmed = Self::capture(roots, choices, targets, original, stop)?;
        if confirmed.feedback != product.feedback {
            return Err(Failure::BackupRootFork.into());
        }
        Ok(confirmed)
    }

    pub(in crate::engine) fn capture(
        roots: &PreparedBackupRoots, choices: &BackupChoiceContext,
        targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<Self> {
        choices.recheck_root_sources(stop)?;
        let mut saved = None;
        for source in choices.sources.values() {
            let value = read(source, choices.intent.operation_id)?
                .ok_or(Failure::BackupRootMissing)?;
            require_source(&value, &choices.transition_hash, choices.intent.operation_id)?;
            merge(&mut saved, value)?;
        }
        let product = Self { feedback: saved.ok_or(Failure::BackupRootMissing)? };
        product.observe_live(roots, choices, targets, original, stop)?;
        choices.recheck_root_sources(stop)?;
        Ok(product)
    }

    pub(in crate::engine) fn capture_decided(
        roots: &PreparedBackupRoots, decision: &super::super::AdmittedBackupDecision,
        targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<Self> {
        let transition = decision.transition();
        let fence = crate::object_hash(
            crate::engine::membership_transition::INTENT_KIND, transition,
        )?;
        let product = Self {
            feedback: Feedback {
                source_fence: SourceFenceHash::from_transition(&fence),
                operation_id: decision.operation(),
                targets: decision.projection().layout_bindings()?,
            },
        };
        if product.feedback.targets.len() != targets.len() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        for index in 0..targets.len() {
            roots.with_decided_layout_source(decision, targets, index, original, stop, |source| {
                product.layout(source.disk())?.observe(source, stop)
            })?;
        }
        Ok(product)
    }

    fn observe_live(
        &self, roots: &PreparedBackupRoots, choices: &BackupChoiceContext,
        targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<()> {
        self.require_source(&choices.transition_hash, choices.intent.operation_id)?;
        if self.feedback.targets.len() != targets.len() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        for index in 0..targets.len() {
            roots.with_layout_source(choices, targets, index, original, stop, |source| {
                self.layout(source.disk())?.observe(source, stop)
            })?;
        }
        Ok(())
    }

    pub(in crate::engine) fn verify(
        &self, roots: &PreparedBackupRoots, evidence: &BackupSourceFenceEvidence, fence: &str,
        targets: &[RetainedBackupTarget], original: &AdmittedState, stop: &AtomicBool,
    ) -> Result<()> {
        if self.feedback.targets.len() != targets.len() {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        for index in 0..targets.len() {
            roots.with_retained_layout_source(self, evidence, fence, targets, index,
                original, stop, |source| self.layout(source.disk())?.observe(source, stop))?;
        }
        Ok(())
    }

    pub(in crate::engine) fn layout(&self, disk: DiskId) -> Result<&BackupTargetLayout> {
        self.feedback.targets.get(&disk).ok_or_else(|| Failure::BackupLayoutEvidence.into())
    }
    pub(super) fn operation(&self) -> OperationId {
        self.feedback.operation_id
    }
    pub(super) fn require_source(&self, fence: &str, operation: OperationId) -> Result<()> {
        require_source(&self.feedback, fence, operation)
    }
}

fn require_source(value: &Feedback, fence: &str, operation: OperationId) -> Result<()> {
    if value.source_fence != SourceFenceHash::from_transition(fence)
        || value.operation_id != operation
    {
        return Err(Failure::BackupLayoutEvidence.into());
    }
    Ok(())
}
fn path(root: &DiskRoot, operation: OperationId) -> std::path::PathBuf {
    root.path().join(layout::BACKUP_LAYOUT_PREPARATIONS.as_path())
        .join(layout::backup_root_preparation(operation).as_path())
}
fn read(source: &Source, operation: OperationId) -> Result<Option<Feedback>> {
    let path = path(&source.root, operation);
    if source.root.optional_metadata(&path)?.is_none() {
        return Ok(None);
    }
    let bytes = source.root.read_bytes(&path, MAX_LAYOUT)?;
    let value: Feedback = decode_strict(&bytes).map_err(Failure::BackupRootRecord)?;
    if encoded(&value)? != bytes {
        return Err(Failure::BackupLayoutEvidence.into());
    }
    Ok(Some(value))
}
fn merge(saved: &mut Option<Feedback>, value: Feedback) -> Result<()> {
    match saved {
        Some(old) if old != &value => Err(Failure::BackupRootFork.into()),
        Some(_) => Ok(()),
        None => { *saved = Some(value); Ok(()) }
    }
}
fn encoded(value: &Feedback) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_LAYOUT {
        return Err(Failure::BackupRootBudget.into());
    }
    Ok(bytes)
}
