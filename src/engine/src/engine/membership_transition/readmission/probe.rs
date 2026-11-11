//! A SYSTEM-retained certificate permits one exact new-lifetime read-only probe.
//! It never supplies a voter or permits serving: recovery still reads actual
//! independent roots and verifies the current configuration's fixed quorum.
use super::*;
use crate::persist::layout;

const DIRECTORY: &str = "readmission-probe-admissions";

#[derive(Serialize, Deserialize)]
enum RecoveryFormat {
    #[serialization(rename = "recovered-readmission-probe")]
    Recovered,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RecoveredProbe {
    format: RecoveryFormat,
    authority: PortableAuthority,
    target: DiskRegistrationIdentity,
    root: std::path::PathBuf,
}
#[derive(Deserialize)]
#[serialization(untagged)]
enum ProbeCertificate {
    Readmission(PortableAuthority),
    Recovered(RecoveredProbe),
}

/// Created only from held resources after explicit recovery authenticated the
/// fixed current quorum. It seeds a new SYSTEM locator, never a new disk epoch.
pub(in crate::engine) struct RecoveredReadmission {
    proof: ReadmissionProbeAdmission,
}
impl RecoveredReadmission {
    pub(in crate::engine) fn request(&self) -> &ReadmissionRequest {
        &self.proof.request
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.proof.operation
    }
    pub(in crate::engine) fn transition(&self) -> &str {
        &self.proof.transition
    }
    pub(in crate::engine) fn root(&self) -> &Path {
        &self.proof.root
    }
}

pub(in crate::engine) struct ReadmissionProbeAdmission {
    request: ReadmissionRequest,
    operation: OperationId,
    transition: String,
    root: std::path::PathBuf,
}
impl ReadmissionProbeAdmission {
    pub(in crate::engine) fn read(system: &Path, transition: &str) -> Result<Self> {
        if !valid_hash(transition) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "invalid readmission probe epoch",
            ));
        }
        let system = DiskRoot::open(system)?;
        let bytes = system.read_bytes(
            &system
                .path()
                .join(DIRECTORY)
                .join(format!("{transition}.json")),
            MAX_METADATA,
        )?;
        let authority = match decode_strict::<ProbeCertificate>(&bytes)? {
            ProbeCertificate::Readmission(authority) => authority,
            ProbeCertificate::Recovered(certificate) => {
                return Self::from_recovered(certificate, transition);
            }
        };
        let intent = authority.intent()?;
        if object_hash(INTENT_KIND, &intent)? != transition {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "readmission probe certificate has another epoch",
            ));
        }
        // intent() closes the complete immutable lineage, original fixed voter
        // set, all member decisions, exact old stop and registry transition.
        let request = match authority.terminal_resume_core() {
            Some(core) => core
                .retained_request()
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "fresh terminal resume cannot authorize a retained probe",
                    )
                })?
                .clone(),
            None if authority.readmission_core().is_some() => request(&intent)?.clone(),
            None => {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "probe certificate is not a retained-directory admission",
                ));
            }
        };
        let root = intent
            .target
            .disks
            .iter()
            .find(|disk| disk.disk_id == request.target.disk_id)
            .expect("invariant: authenticated readmission target")
            .root
            .clone();
        Ok(Self {
            request,
            operation: intent.operation_id,
            transition: transition.into(),
            root,
        })
    }

    fn from_recovered(certificate: RecoveredProbe, transition: &str) -> Result<Self> {
        let (intent, request) = certificate
            .authority
            .readmission_epoch(&certificate.target, transition)?;
        Ok(Self {
            request,
            operation: intent.operation_id,
            transition: transition.to_owned(),
            root: certificate.root,
        })
    }

    pub(in crate::engine) fn admits(
        &self,
        identity: &DiskRegistrationIdentity,
        root: &Path,
        physical: FileIdentity,
        operation: OperationId,
        transition: &str,
        stopped: &str,
    ) -> bool {
        self.request.target == *identity
            && self.root == root
            && self.request.directory.device == physical.device
            && self.request.directory.inode == physical.inode
            && self.operation == operation
            && self.transition == transition
            && self.request.stopped_transition == stopped
    }
}

pub(in crate::engine::membership_transition) fn persist_terminal_probe_admission(
    system: &Path,
    authority: &PortableAuthority,
    admission: &crate::engine::membership_transition::terminal_resume::AdmittedTerminalReadmission,
) -> Result<()> {
    let intent = authority.intent()?;
    if authority.hash()? != admission.generation()
        || admission.operation() != intent.operation_id
        || admission.transition() != object_hash(INTENT_KIND, &intent)?
        || authority
            .terminal_resume_core()
            .and_then(|core| core.retained_request())
            .is_none_or(|request| request.target != admission.request().target)
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "terminal probe differs from its actual independent member admission",
        ));
    }
    let system = DiskRoot::open(system)?;
    let directory = system.path().join(DIRECTORY);
    system.private_dir(&directory)?;
    system.write_json(
        &directory.join(format!("{}.json", admission.transition())),
        authority,
        false,
    )
}

pub(in crate::engine::membership_transition) fn persist_probe_admission(
    system: &Path,
    authority: &PortableAuthority,
    admission: &AdmittedReadmission,
) -> Result<()> {
    let intent = authority.intent()?;
    if admission.transition() != object_hash(INTENT_KIND, &intent)?
        || admission.operation() != intent.operation_id
    {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "probe certificate differs from the live admitted readmission",
        ));
    }
    let system = DiskRoot::open(system)?;
    let directory = system.path().join(DIRECTORY);
    system.private_dir(&directory)?;
    // A retry verifies the immutable bytes and repeats both durability barriers.
    system.write_json(
        &directory.join(format!("{}.json", admission.transition())),
        authority,
        false,
    )
}

/// The caller has already authenticated the actual fixed metadata quorum and
/// preflighted recovery while retaining its directory handles and writer locks.
/// Persist only the explicitly supplied current roots, never historical paths.
pub(in crate::engine) fn persist_recovered_admissions(
    system: &Path,
    config: &ValidatedConfig,
    resources: &DiskResources,
) -> Result<Vec<RecoveredReadmission>> {
    let Some(generation) = config.generation() else {
        return Ok(Vec::new());
    };
    let mut recovered = Vec::new();
    for handle in resources.handles() {
        let disk = config
            .io_disks()
            .find(|disk| disk.disk_id == handle.seal.disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "recovered epoch resource is outside admitted configuration",
                )
            })?;
        let Some(epoch) = config.io_admission_epoch(&disk.disk_id)? else {
            continue;
        };
        let identity = DiskRegistrationIdentity {
            pool_id: config.pool_id.clone(),
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        };
        let (_, request) = generation.portable.readmission_epoch(&identity, &epoch)?;
        request.directory.verify_physical_root(&handle.root)?;
        let expected = identity_at(config, disk, &handle.root.directory(handle.root.path())?)?;
        let stored = decode_seal(
            &handle
                .root
                .read_bytes(&disk.root.join(layout::DISK_IDENTITY.as_path()), MAX_METADATA)?,
        )?;
        validate_stored_seal(&stored, &expected)?;
        let certificate = RecoveredProbe {
            format: RecoveryFormat::Recovered,
            authority: generation.portable.clone(),
            target: identity,
            root: disk.root.clone(),
        };
        PreparedMetadataObject::new("recovered-readmission-probe", &certificate, MAX_METADATA)?;
        let root = DiskRoot::open(system)?;
        let directory = root.path().join(DIRECTORY);
        root.private_dir(&directory)?;
        root.write_json(
            &directory.join(format!("{epoch}.json")),
            &certificate,
            false,
        )?;
        recovered.push(RecoveredReadmission {
            proof: ReadmissionProbeAdmission::from_recovered(certificate, &epoch)?,
        });
    }
    Ok(recovered)
}
