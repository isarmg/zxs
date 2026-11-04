//! Durable joint decisions while every business path remains fenced. Nothing in
//! this namespace is an active-generation journal or a normal operation result.
#[cfg(feature = "fault-injection")]
use super::super::decision::DecisionFault;
use super::super::decision::{DecisionReceipt, DecisionStore};
use super::*;
use crate::metadata_map::{Encoder, ProofMeasure, Reference};
use std::collections::BTreeSet;

pub(super) mod store;
use store::{JointStore, PreparedProposal};

const PROPOSAL_KIND: &str = "membership-joint-proposal";
const ACCEPTANCE_KIND: &str = "membership-joint-acceptance";
const CERTIFICATE_KIND: &str = "membership-joint-certificate";
const BINDING_FILE: &str = "proposal-binding.json";
const CERTIFICATE_FILE: &str = "certificate.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipJointRole {
    OldOnly,
    Retained,
    NewOnly,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipJointVote {
    pub disk_id: String,
    pub slot_id: String,
    pub binding_generation: u64,
    pub role: MembershipJointRole,
    pub acceptance_hash: String,
    pub decision_hash: String,
}
/// Synchronized observations from the latest explicit joint attempt. A failed
/// attempt clears previous observations first; this is not a background health
/// check and quorum is not target installation, activation, or write permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipJointDecisionStatus {
    pub transition_hash: String,
    pub proposal_hash: String,
    pub original_members: usize,
    pub original_quorum: usize,
    pub synchronized_original_votes: usize,
    pub target_members: usize,
    pub target_quorum: usize,
    pub union_members: usize,
    pub added_data_disks: usize,
    pub ready_data_disks: Vec<String>,
    pub synchronized_target_votes: usize,
    pub votes: Vec<MembershipJointVote>,
    pub certificate_hash: Option<String>,
    pub certificate_members: Vec<String>,
}
impl MembershipJointDecisionStatus {
    pub fn original_quorum_accepted(&self) -> bool {
        self.synchronized_original_votes >= self.original_quorum
    }
    pub fn target_quorum_accepted(&self) -> bool {
        self.synchronized_target_votes >= self.target_quorum
    }
    pub fn joint_quorum_accepted(&self) -> bool {
        self.original_quorum_accepted() && self.target_quorum_accepted()
    }
    /// Counts actual durable certificate locations by their independently bound role.
    /// Missing old-only copies are a coverage gap, not a changed quorum.
    pub fn certificate_quorum_counts(&self) -> (usize, usize) {
        let mut old = 0;
        let mut target = 0;
        for vote in self
            .votes
            .iter()
            .filter(|vote| self.certificate_members.contains(&vote.disk_id))
        {
            match vote.role {
                MembershipJointRole::OldOnly => old += 1,
                MembershipJointRole::Retained => {
                    old += 1;
                    target += 1;
                }
                MembershipJointRole::NewOnly => target += 1,
            }
        }
        (old, target)
    }
    /// Full coverage observation, stronger than the fixed two-sided certificate rule.
    pub fn all_certificates_synchronized(&self) -> bool {
        self.joint_quorum_accepted()
            && self.synchronized_target_votes == self.target_members
            && self.certificate_members.len() == self.union_members
            && self.ready_data_disks.len() == self.added_data_disks
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum JointFormat {
    #[serialization(rename = "fenced-joint-slot-decision")]
    Joint,
    #[serialization(rename = "fenced-device-slot-decision")]
    Devices,
    #[serialization(rename = "fenced-exit-control-slot-decision")]
    ExitControl,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Proposal {
    format: JointFormat,
    transition_hash: String,
    original_config_hash: String,
    target_config_hash: String,
    pub(super) control_archive_root: String,
    operation_id: OperationId,
    principal: String,
    frozen_boundary: FrozenBoundary,
    #[serialization(with = "storage_contract::u64_string")]
    fence_sequence: u64,
    original_members: Reference,
    target_members: Reference,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    data_disks: Option<Reference>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    old_only_members: Option<Reference>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct OriginalDependency {
    identity: MemberIdentity,
    fence: Receipt,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct TargetDependency {
    pub(super) enrollment: MemberEnrollment,
    bootstrap: ControlBootstrap,
    seal: MemberSeal,
    control_receipt: Receipt,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum MemberSeal {
    Retained { seal: DiskSeal },
    Added { seal: PendingDiskSeal },
}

#[cfg(feature = "fault-injection")]
pub(super) struct JointFault {
    pub(super) disk_id: String,
    pub(super) stage: JointFaultStage,
}
#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy)]
pub(super) enum JointFaultStage {
    Decision(DecisionFault),
    Completion(OpeningFault),
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Acceptance {
    proposal_hash: String,
    identity: MemberIdentity,
    enrollment_hash: String,
    bootstrap_hash: String,
    control_receipt: Receipt,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct JointBinding {
    identity: MemberIdentity,
    proposal_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct JointVote {
    identity: MemberIdentity,
    acceptance_hash: String,
    decision: DecisionReceipt,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Certificate {
    format: JointFormat,
    proposal_hash: String,
    votes: Reference,
}

pub(super) struct JointProgress {
    pub(super) status: MembershipJointDecisionStatus,
    original: BTreeSet<(String, String, u64)>,
    target: BTreeSet<(String, String, u64)>,
}
impl JointProgress {
    fn new(
        pending: &PendingMembership,
        proposal: &PreparedProposal,
        fence: &AdmittedFence,
    ) -> Self {
        let config = fence.intent.origin.config();
        Self {
            original: config
                .metadata_members()
                .map(|disk| {
                    (
                        disk.disk_id.clone(),
                        disk.slot_id.clone(),
                        disk.binding_generation,
                    )
                })
                .collect(),
            target: fence
                .intent
                .target
                .metadata_members()
                .map(|disk| {
                    (
                        disk.disk_id.clone(),
                        disk.slot_id.clone(),
                        disk.binding_generation,
                    )
                })
                .collect(),
            status: MembershipJointDecisionStatus {
                transition_hash: pending.transition_hash.clone(),
                proposal_hash: proposal.object.hash.clone(),
                original_members: config.metadata_members().len(),
                original_quorum: config.metadata_quorum(),
                synchronized_original_votes: 0,
                target_members: fence.intent.target.metadata_members().len(),
                union_members: pending.members.len(),
                target_quorum: fence.intent.target.metadata_quorum(),
                added_data_disks: pending.data_disks.len(),
                ready_data_disks: pending.ready_data_disks.clone(),
                synchronized_target_votes: 0,
                votes: Vec::new(),
                certificate_hash: None,
                certificate_members: Vec::new(),
            },
        }
    }
    fn observe(&mut self, vote: &JointVote) {
        let identity = &vote.identity;
        let member = (
            identity.disk_id.clone(),
            identity.slot_id.clone(),
            identity.binding_generation,
        );
        let old = self.original.contains(&member);
        let target = self.target.contains(&member);
        if old {
            self.status.synchronized_original_votes += 1;
        }
        if target {
            self.status.synchronized_target_votes += 1;
        }
        let role = match (old, target) {
            (true, false) => MembershipJointRole::OldOnly,
            (true, true) => MembershipJointRole::Retained,
            (false, true) => MembershipJointRole::NewOnly,
            (false, false) => unreachable!("invariant: admitted union participant"),
        };
        self.status.votes.push(MembershipJointVote {
            disk_id: identity.disk_id.clone(),
            slot_id: identity.slot_id.clone(),
            binding_generation: identity.binding_generation,
            role,
            acceptance_hash: vote.acceptance_hash.clone(),
            decision_hash: vote.decision.hash().to_owned(),
        });
    }
}

struct CheckedMember {
    directory: DiskRoot,
    archive: DiskRoot,
    bootstrap: ControlBootstrap,
}
impl CheckedMember {
    fn verify_names(&self, member: &PendingMember, joint: Option<&JointStore>) -> Result<()> {
        verify_receipt_directories(member, &self.directory, &self.archive)?;
        if let Some(joint) = joint {
            verify_child(&self.directory, &joint.root)?;
            joint.decision.verify_names()?;
        }
        Ok(())
    }
}
fn verify_child(parent: &DiskRoot, child: &DiskRoot) -> Result<()> {
    if FileIdentity::from_fd(parent.directory(child.path())?)?
        != FileIdentity::from_fd(child.directory(child.path())?)?
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "joint directory moved before durable receipt",
        ));
    }
    Ok(())
}

impl Engine {
    /// Deterministic test injection; never configured by requests or environment.
    #[cfg(feature = "fault-injection")]
    pub fn inject_membership_joint_fault_once(&mut self, disk_id: &str, point: &str) -> Result<()> {
        let stage = match point {
            "partial-write" => JointFaultStage::Decision(DecisionFault::StagingWrite),
            "journal-sync" => JointFaultStage::Decision(DecisionFault::PreparedFileSync),
            "opening-file-sync" => JointFaultStage::Decision(DecisionFault::FileSync),
            "opening-directory-sync" => JointFaultStage::Decision(DecisionFault::DirectorySync),
            "decision-no-replace" => JointFaultStage::Decision(DecisionFault::NoReplace),
            "decision-published-reply" => JointFaultStage::Decision(DecisionFault::PublishedReply),
            "decision-unknown-publication" => {
                JointFaultStage::Decision(DecisionFault::UnknownPublication)
            }
            "decision-published-durability" => {
                JointFaultStage::Decision(DecisionFault::PublishedDurability)
            }
            "completion-file-sync" => JointFaultStage::Completion(OpeningFault::FileSync),
            "completion-directory-sync" => JointFaultStage::Completion(OpeningFault::DirectorySync),
            _ => {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "unknown joint fault point",
                ));
            }
        };
        let pending = self.pending_membership.as_mut().ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "no admitted pending members")
        })?;
        if !pending
            .members
            .iter()
            .any(|member| member.disk.disk_id == disk_id)
        {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "joint fault target is not an admitted member",
            ));
        }
        pending.joint_fault = Some(JointFault {
            disk_id: disk_id.to_owned(),
            stage,
        });
        Ok(())
    }

    pub fn membership_joint_decision_status(&self) -> Option<MembershipJointDecisionStatus> {
        self.pending_membership
            .as_ref()?
            .joint
            .as_ref()
            .map(|joint| joint.status.clone())
    }

    /// Persist/resume the one proposal derived from the admitted original fence,
    /// trusted archive and actual local control receipts. No caller-supplied vote
    /// is accepted. Reopen from original members, explicitly enroll the same
    /// pending roots, then call this method after loss of the system directory.
    /// Successful completion still leaves the original Engine/config blocked.
    pub fn resume_membership_joint_decision(&mut self) -> Result<MembershipJointDecisionStatus> {
        self.run_joint_decision(true)
    }

    /// Reverify complete installed certificates without filling missing objects,
    /// changing named decisions or appending votes. Actual decisions and the completion
    /// file/parent are resynchronized; dependency objects are read and verified,
    /// relying on the barriers preceding the original vote/root. This fails on missing local
    /// closure even if another target or the source archive has the object.
    pub fn verify_membership_joint_decision(&mut self) -> Result<MembershipJointDecisionStatus> {
        self.run_joint_decision(false)
    }

    fn run_joint_decision(&mut self, install: bool) -> Result<MembershipJointDecisionStatus> {
        let mut pending = self.pending_membership.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "joint decision requires explicitly admitted pending members",
            )
        })?;
        pending.joint = None;
        pending.receipts.clear();
        pending.ready_data_disks.clear();
        let result = self.joint_decision(&mut pending, install);
        let status = pending.joint.as_ref().map(|joint| joint.status.clone());
        self.pending_membership = Some(pending);
        result?;
        Ok(status.expect("invariant: successful joint decision establishes status"))
    }

    fn joint_decision(&mut self, pending: &mut PendingMembership, install: bool) -> Result<()> {
        self.validate_pending_target()?;
        pending
            .source
            .verify_current_fence(self)
            .map_err(|error| joint_error(error, "source/original fence", None))?;
        pending.verify_data_readiness()?;
        pending.ready_data_disks = pending
            .data_disks
            .iter()
            .map(|disk| disk.disk.disk_id.clone())
            .collect();
        let checked = self.check_joint_controls(pending)?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: pending target requires fence");
        let proposal = PreparedProposal::new(pending, fence, &checked)?;
        pending.joint = Some(JointProgress::new(pending, &proposal, fence));
        // Hold every original writer lock and inspect every shared choice before
        // any new Joint vote. Even a visible minority Abort vetoes continuation.
        for member in &pending.members {
            DecisionStore::open(
                member.resource.root(),
                &member.enrollment.identity,
                &pending.transition_hash,
                install,
            )
            .and_then(|store| store.require_joint_allowed())
            .map_err(|error| joint_error(error, "decision preflight", Some(&member.disk)))?;
        }
        let mut stores = Vec::new();
        let mut votes = BTreeMap::new();
        for (member, checked) in pending.members.iter().zip(&checked) {
            let result = (|| {
                checked.verify_names(member, None)?;
                let store = JointStore::open(
                    member.resource.root(),
                    &checked.directory,
                    &member.enrollment.identity,
                    &pending.transition_hash,
                    &proposal.object.hash,
                    install,
                )?;
                #[cfg(feature = "fault-injection")]
                self.failpoint(&format!(
                    "after_membership_joint_binding_{}",
                    member.disk.disk_id
                ))?;
                if install {
                    store.install(&proposal.objects)?;
                }
                store.verify_proposal(&proposal.object.hash)?;
                #[cfg(feature = "fault-injection")]
                self.failpoint(&format!(
                    "after_membership_joint_dependencies_{}",
                    member.disk.disk_id
                ))?;
                checked.verify_names(member, Some(&store))?;
                let acceptance = &proposal.acceptances[&member.disk.disk_id];
                #[cfg(feature = "fault-injection")]
                let fault = match &pending.joint_fault {
                    Some(JointFault {
                        disk_id,
                        stage: JointFaultStage::Decision(fault),
                    }) if disk_id == &member.disk.disk_id => {
                        let fault = Some(*fault);
                        pending.joint_fault = None;
                        fault
                    }
                    _ => None,
                };
                let decision = store.decision.joint_receipt(
                    &proposal.object.hash,
                    &acceptance.hash,
                    install,
                    #[cfg(feature = "fault-injection")]
                    fault,
                )?;
                checked.verify_names(member, Some(&store))?;
                let vote = JointVote {
                    identity: member.enrollment.identity.clone(),
                    acceptance_hash: acceptance.hash.clone(),
                    decision,
                };
                Ok((store, vote))
            })();
            let (store, vote) =
                result.map_err(|error| joint_error(error, "accept", Some(&member.disk)))?;
            pending
                .joint
                .as_mut()
                .expect("invariant: proposal established")
                .observe(&vote);
            votes.insert(member.disk.disk_id.clone(), vote);
            stores.push(store);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_joint_vote_{}", votes.len()))?;
        }
        pending.verify_data_readiness()?;
        let (certificate, objects) = proposal.certificate(&votes)?;
        pending
            .joint
            .as_mut()
            .expect("invariant: proposal established")
            .status
            .certificate_hash = Some(certificate.hash.clone());
        for ((member, checked), store) in pending.members.iter().zip(&checked).zip(&stores) {
            let result = (|| {
                checked.verify_names(member, Some(store))?;
                if install {
                    store.install(&objects)?;
                }
                // All certificate dependency reads below are from this store.
                store.verify_certificate(&certificate.hash, &proposal.object.hash, &member.disk)?;
                // Recheck the held local archive, bootstrap and exact decision,
                // rather than using the earlier in-memory observation as proof.
                self.check_joint_control(member, &pending.source)?;
                checked.verify_names(member, Some(store))?;
                #[cfg(feature = "fault-injection")]
                self.failpoint(&format!(
                    "before_membership_joint_completion_{}",
                    member.disk.disk_id
                ))?;
                #[cfg(feature = "fault-injection")]
                let completion_fault = match &pending.joint_fault {
                    Some(JointFault {
                        disk_id,
                        stage: JointFaultStage::Completion(fault),
                    }) if disk_id == &member.disk.disk_id => {
                        let fault = Some(*fault);
                        pending.joint_fault = None;
                        fault
                    }
                    _ => None,
                };
                store.synchronize_certificate(
                    &certificate,
                    install,
                    #[cfg(feature = "fault-injection")]
                    completion_fault,
                )?;
                #[cfg(feature = "fault-injection")]
                self.failpoint(&format!(
                    "after_membership_joint_completion_sync_{}",
                    member.disk.disk_id
                ))?;
                checked.verify_names(member, Some(store))?;
                Ok(())
            })();
            result.map_err(|error| joint_error(error, "certificate", Some(&member.disk)))?;
            pending
                .joint
                .as_mut()
                .expect("invariant: proposal established")
                .status
                .certificate_members
                .push(member.disk.disk_id.clone());
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_joint_certificate_{}",
                pending
                    .joint
                    .as_ref()
                    .expect("invariant: proposal established")
                    .status
                    .certificate_members
                    .len()
            ))?;
        }
        if let Err(error) = pending.verify_data_readiness() {
            pending
                .joint
                .as_mut()
                .expect("invariant: proposal established")
                .status
                .ready_data_disks
                .clear();
            return Err(error);
        }
        // A prior member may have moved while a later certificate was being
        // installed. Do not turn the earlier observations into global success.
        for ((member, checked), store) in pending.members.iter().zip(&checked).zip(&stores) {
            let result = checked.verify_names(member, Some(store)).and_then(|()| {
                store
                    .decision
                    .verify_receipt(&votes[&member.disk.disk_id].decision, false)
            });
            if let Err(error) = result {
                pending
                    .joint
                    .as_mut()
                    .expect("invariant: proposal established")
                    .status
                    .certificate_members
                    .retain(|id| id != &member.disk.disk_id);
                return Err(joint_error(
                    error,
                    "final member decision",
                    Some(&member.disk),
                ));
            }
        }
        Ok(())
    }

    fn check_joint_controls(&self, pending: &mut PendingMembership) -> Result<Vec<CheckedMember>> {
        let mut checked = Vec::new();
        for member in &pending.members {
            let (held, receipt) = self
                .check_joint_control(member, &pending.source)
                .map_err(|error| joint_error(error, "control", Some(&member.disk)))?;
            pending
                .receipts
                .insert(member.disk.disk_id.clone(), receipt);
            checked.push(held);
        }
        Ok(checked)
    }

    fn check_joint_control(
        &self,
        member: &PendingMember,
        source: &PendingControlArchive,
    ) -> Result<(CheckedMember, Receipt)> {
        self.validate_pending_resource(member)?;
        let path = member_directory(&member.disk, &member.enrollment.transition_hash);
        let directory = member.resource.root().private_child(&path)?;
        let saved: MemberEnrollment =
            decode_strict(&directory.read_bytes(&path.join("member.json"), MAX_METADATA)?)?;
        if saved != member.enrollment {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint control enrollment differs",
            ));
        }
        let bytes = directory.read_bytes(
            &path
                .join("objects")
                .join(BOOTSTRAP_KIND)
                .join(format!("{}.json", member.bootstrap.hash)),
            MAX_METADATA,
        )?;
        let bootstrap = json_codec::from_value(
            AuthenticatedMetadataObject::authenticate(
                BOOTSTRAP_KIND,
                &member.bootstrap.hash,
                decode_strict(&bytes)?,
            )?
            .into_value(),
        )?;
        let archive = source
            .verify_installed(directory.private_child(&path.join(CONTROL_ARCHIVE_DIRECTORY))?)?;
        let checked = CheckedMember {
            directory,
            archive,
            bootstrap,
        };
        checked.verify_names(member, None)?;
        let sequence = self
            .membership_fence
            .as_ref()
            .expect("invariant: pending resource retains fence")
            .intent
            .fence_sequence;
        let mut journal =
            PendingJournal::open_at(&checked.directory, &path, sequence, &member.bootstrap.hash)?;
        // Control enrollment must already be complete. Only explicit pending
        // resume may install its missing objects or repair its incomplete frame.
        journal.require_complete()?;
        let receipt = journal.synchronize(&member.disk)?;
        checked.verify_names(member, None)?;
        Ok((checked, receipt))
    }
}

fn joint_error(error: Error, stage: &str, disk: Option<&DiskConfig>) -> Error {
    let target = disk.map_or_else(
        || "source".into(),
        |disk| {
            format!(
                "{}/{} generation {}",
                disk.disk_id, disk.slot_id, disk.binding_generation
            )
        },
    );
    let code = error.code();
    error.context(code, format!("membership joint {stage}: {target}"))
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_joint_tests.rs"]
mod tests;
