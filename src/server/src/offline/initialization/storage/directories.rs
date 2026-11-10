//! Create only the selected empty first-storage directories. The existing
//! storage initializer still owns genesis, credentials and all database writes.
mod entries;

use super::{Root, layout};
use crate::offline::Result;
use data_serialization::{Deserialize, Serialize};
use platform_io::publication::BundleState;
use sha2::{Digest, Sha256};

const PLAN_DIRECTORY: &str = ".zxs-helper-first-storage-directories";
const COMPLETION_DIRECTORY: &str = ".zxs-helper-first-storage-directories-complete";
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    pub service_user: String,
    pub roots: Vec<String>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Prepare {
    pub input: Input,
    pub request_digest: String,
    pub confirmation: String,
    pub external_writers_stopped: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-first-storage-directories")]
    Directories,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Identity {
    device: String,
    inode: String,
}
impl From<platform_io::fs_safety::linux::FileIdentity> for Identity {
    fn from(identity: platform_io::fs_safety::linux::FileIdentity) -> Self {
        Self {
            device: identity.device.to_string(),
            inode: identity.inode.to_string(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Target {
    path: String,
    original: Option<Identity>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Plan {
    format: Format,
    revision: u32,
    input: Input,
    context: layout::Context,
    targets: Vec<Target>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    request_digest: String,
    path: String,
    identity: Identity,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completion {
    format: Format,
    revision: u32,
    request_digest: String,
    roots: Vec<Binding>,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    request_digest: String,
    input: Input,
    directories: Vec<entries::Observation>,
    original_plan_recorded: bool,
    original_completion_present: bool,
    creates_only_missing_empty_directories: bool,
    changes_existing_permissions: bool,
    initializes_storage: bool,
    starts_service: bool,
}
#[derive(Serialize)]
pub(crate) struct Outcome {
    state: &'static str,
    request_digest: String,
    roots: Vec<Binding>,
    storage_initialized: bool,
    service_started: bool,
}
struct Prepared {
    root: Root,
    plan: Plan,
    digest: String,
    plan_phase: BundleState,
}
fn bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = json_codec::to_vec(value)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("first-storage directory record exceeds its fixed budget".into());
    }
    Ok(bytes)
}
impl Prepared {
    fn open(input: Input) -> Result<Self> {
        if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
            return Err(
                "first-storage directory preparation requires the manually run root helper".into(),
            );
        }
        layout::names(&input.roots)?;
        let root = Root::open(input.service_user.clone())?;
        let (parent, owner) = root.layout.first_storage_directory_parent()?;
        let retained = root
            .host
            .read_operation(PLAN_DIRECTORY, "plan.json", MAX_RECORD_BYTES)?;
        let plan = match retained {
            Some(bytes) => json_codec::from_slice::<Plan>(&bytes)?,
            None => Plan {
                format: Format::Directories,
                revision: 1,
                context: root.context.clone(),
                targets: entries::capture(parent, owner, &input.roots)?,
                input: input.clone(),
            },
        };
        if plan.revision != 1
            || plan.input != input
            || plan.context != root.context
            || plan.targets.len() != input.roots.len()
            || plan
                .targets
                .iter()
                .zip(&input.roots)
                .any(|(target, path)| &target.path != path)
        {
            return Err("first-storage directory request or native layout differs from the original; no new batch was substituted".into());
        }
        let encoded = bytes(&plan)?;
        let digest = platform_io::lower_hex(&Sha256::digest(&encoded));
        let plan_phase = root
            .host
            .inspect(PLAN_DIRECTORY, &[("plan.json", &encoded)])?;
        Ok(Self {
            root,
            plan,
            digest,
            plan_phase,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.root.host.recheck()?;
        self.root.layout.first_storage_directory_parent()?;
        if self.root.layout.context(self.root.host.identity()) != self.plan.context {
            return Err("first-storage directory parent or account identity changed".into());
        }
        Ok(())
    }
    fn observe(&self) -> Result<Vec<entries::Entry>> {
        self.recheck()?;
        let (parent, owner) = self.root.layout.first_storage_directory_parent()?;
        let observed = entries::observe(
            &self.root.host,
            parent,
            owner,
            &self.plan.targets,
            &self.digest,
        )?;
        if self.plan_phase != BundleState::Published
            && observed
                .iter()
                .zip(&self.plan.targets)
                .any(|(entry, target)| target.original.is_none() && !entry.is_absent())
        {
            return Err(
                "new directory material exists before its complete original plan; it was preserved"
                    .into(),
            );
        }
        self.recheck()?;
        Ok(observed)
    }
    fn completion(&self, entries: &[entries::Entry]) -> Result<Option<Completion>> {
        let complete = entries
            .iter()
            .map(entries::Entry::completed)
            .collect::<Option<Vec<_>>>();
        let expected = complete.map(|roots| Completion {
            format: Format::Directories,
            revision: 1,
            request_digest: self.digest.clone(),
            roots,
        });
        let retained = self.root.host.read_operation(
            COMPLETION_DIRECTORY,
            "receipt.json",
            MAX_RECORD_BYTES,
        )?;
        let encoded = match (&expected, retained) {
            (Some(expected), Some(retained)) => {
                let actual: Completion = json_codec::from_slice(&retained)?;
                if actual != *expected {
                    return Err("first-storage directory completion no longer matches the original inodes; nothing was recreated".into());
                }
                bytes(expected)?
            }
            (None, Some(_)) => {
                return Err("a completed first-storage directory is missing or unfinished; no directory was recreated".into());
            }
            (Some(expected), None) => bytes(expected)?,
            (None, None) => Vec::new(),
        };
        let phase = self
            .root
            .host
            .inspect(COMPLETION_DIRECTORY, &[("receipt.json", &encoded)])?;
        if phase != BundleState::Absent && expected.is_none() {
            return Err(
                "directory completion began but its original directory set is no longer complete"
                    .into(),
            );
        }
        if phase == BundleState::Published {
            Ok(expected)
        } else {
            Ok(None)
        }
    }
    fn preview(self) -> Result<Preview> {
        let observed = self.observe()?;
        let completed = self.completion(&observed)?.is_some();
        Ok(Preview {
            request_digest: self.digest,
            input: self.plan.input,
            directories: observed.iter().map(entries::Entry::observation).collect(),
            original_plan_recorded: self.plan_phase == BundleState::Published,
            original_completion_present: completed,
            creates_only_missing_empty_directories: true,
            changes_existing_permissions: false,
            initializes_storage: false,
            starts_service: false,
        })
    }
    fn publish(mut self) -> Result<Outcome> {
        let original = self.observe()?;
        self.completion(&original)?;
        let encoded = bytes(&self.plan)?;
        self.root
            .host
            .publish(PLAN_DIRECTORY, &[("plan.json", &encoded)])?;
        self.plan_phase = BundleState::Published;
        for target in &self.plan.targets {
            self.recheck()?;
            let (parent, owner) = self.root.layout.first_storage_directory_parent()?;
            entries::publish(&self.root.host, parent, owner, target, &self.digest, || {
                self.recheck()
            })?;
            self.observe()?;
        }
        let final_entries = self.observe()?;
        let roots = final_entries
            .iter()
            .map(entries::Entry::completed)
            .collect::<Option<Vec<_>>>()
            .ok_or("first-storage directories are still incomplete")?;
        let receipt = Completion {
            format: Format::Directories,
            revision: 1,
            request_digest: self.digest.clone(),
            roots,
        };
        let encoded = bytes(&receipt)?;
        self.root
            .host
            .publish(COMPLETION_DIRECTORY, &[("receipt.json", &encoded)])?;
        let final_entries = self.observe()?;
        let completed = self
            .completion(&final_entries)?
            .ok_or("first-storage directory completion was not durably published")?;
        Ok(Outcome {
            state: "original_empty_storage_directories_durably_prepared",
            request_digest: self.digest,
            roots: completed.roots,
            storage_initialized: false,
            service_started: false,
        })
    }
}
pub(crate) fn preview(input: Input) -> Result<Preview> {
    Prepared::open(input)?.preview()
}
pub(crate) fn prepare(input: Prepare) -> Result<Outcome> {
    if input.confirmation != "prepare-first-storage-directories-only"
        || !input.external_writers_stopped
    {
        return Err("explicit empty-directory preparation and stopped external writers are required before host access".into());
    }
    let prepared = Prepared::open(input.input)?;
    if prepared.digest != input.request_digest {
        return Err(
            "first-storage directory request differs from its exact preview; nothing was created"
                .into(),
        );
    }
    let request_digest = prepared.digest.clone();
    prepared.publish().map_err(|source| {
        Box::new(Pending {
            request_digest,
            source,
        }) as Box<dyn std::error::Error + Send + Sync>
    })
}
#[derive(Debug)]
struct Pending {
    request_digest: String,
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "first-storage directory preparation {} may be partly or fully published; retain its exact original input, inode records and pending names; only explicitly continue this request after inspecting the cause; no pool initialization or service readiness is claimed: {}",
            self.request_digest, self.source
        )
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
