//! Held SYSTEM storage for immutable activation plans and their selected heads.
use super::*;
use crate::persist::layout;

const HEAD: &str = "head.json";
pub(super) const PLAN: &str = "plan.json";
const COMPLETE: &str = "complete.json";

pub(super) trait Plan: Serialize + for<'de> Deserialize<'de> {
    type Head: Serialize + for<'de> Deserialize<'de> + PartialEq;
    const DIRECTORY: &'static str;
    const LABEL: &'static str;

    fn head(&self) -> Result<Self::Head>;
    fn operation_id(head: &Self::Head) -> OperationId;
}

pub(super) struct PlanStore<P: Plan> {
    system: DiskRoot,
    directory: DiskRoot,
    operation: DiskRoot,
    pub(super) head: P::Head,
}

impl<P: Plan> PlanStore<P> {
    pub(super) fn open(root: &Path) -> Result<Option<(Self, P)>> {
        let system = DiskRoot::open(root)?;
        let path = system.path().join(P::DIRECTORY);
        if system.optional_metadata(&path)?.is_none() {
            return Ok(None);
        }
        let directory = system.private_child(&path)?;
        Self::inventory(&directory, true)?;
        let Some(head) = optional::<P::Head>(&directory, HEAD)? else {
            // A pre-head write has not been allowed to publish any locator.
            return Ok(None);
        };
        let operation =
            directory.private_child(&directory.path().join(P::operation_id(&head).to_string()))?;
        Self::inventory(&operation, false)?;
        let plan: P =
            decode_strict(&operation.read_bytes(&operation.path().join(PLAN), MAX_METADATA)?)?;
        if plan.head()? != head {
            return Err(Self::conflict("plan differs from its protected head"));
        }
        Ok(Some((
            Self {
                system,
                directory,
                operation,
                head,
            },
            plan,
        )))
    }

    pub(super) fn prepare(engine: &Engine, plan: &P) -> Result<Self> {
        let system = DiskRoot::open(&engine.root)?;
        Self::verify_writer(&system, engine)?;
        system.private_dir(&system.path().join(P::DIRECTORY))?;
        let directory = system.private_child(&system.path().join(P::DIRECTORY))?;
        Self::inventory(&directory, true)?;
        let head = plan.head()?;
        let path = directory.path().join(P::operation_id(&head).to_string());
        directory.private_dir(&path)?;
        let operation = directory.private_child(&path)?;
        Self::inventory(&operation, false)?;
        // Immutable bytes first. An interrupted old operation cannot be silently
        // replaced by a different authority or a new observation of its roots.
        operation.write_json(&operation.path().join(PLAN), plan, false)?;
        directory.write_json(&directory.path().join(HEAD), &head, true)?;
        Ok(Self {
            system,
            directory,
            operation,
            head,
        })
    }

    pub(super) fn verify_current(&self, engine: &Engine) -> Result<P> {
        Self::verify_writer(&self.system, engine)?;
        Self::inventory(&self.directory, true)?;
        Self::inventory(&self.operation, false)?;
        let current: P::Head = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(HEAD), MAX_METADATA)?,
        )?;
        let plan: P = decode_strict(
            &self
                .operation
                .read_bytes(&self.operation.path().join(PLAN), MAX_METADATA)?,
        )?;
        if current != self.head || plan.head()? != self.head {
            return Err(Self::conflict(
                "plan changed before its publication barrier",
            ));
        }
        Ok(plan)
    }

    pub(super) fn synchronize(&self, engine: &Engine) -> Result<()> {
        let plan = self.verify_current(engine)?;
        // Existing exact bytes still require today's file and directory barriers.
        self.operation
            .write_json(&self.operation.path().join(PLAN), &plan, false)?;
        self.directory
            .write_json(&self.directory.path().join(HEAD), &self.head, false)
    }

    pub(super) fn completed(&self) -> Result<bool> {
        match optional::<P::Head>(&self.operation, COMPLETE)? {
            None => Ok(false),
            Some(head) if head == self.head => Ok(true),
            Some(_) => Err(Self::conflict("completion differs from its exact plan")),
        }
    }

    pub(super) fn complete(&self) -> Result<()> {
        self.operation
            .write_json(&self.operation.path().join(COMPLETE), &self.head, false)
    }

    fn conflict(detail: &str) -> Error {
        Error::new(ErrorCode::LogFork, format!("{} {detail}", P::LABEL))
    }

    fn verify_writer(system: &DiskRoot, engine: &Engine) -> Result<()> {
        let named = system.open_regular_entry(&layout::CONTROL_WRITER_LOCK)?;
        if FileIdentity::from_fd(&named)? != engine._locks.system_identity()? {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                format!("{} lost its protected system writer", P::LABEL),
            ));
        }
        Ok(())
    }

    fn inventory(root: &DiskRoot, top: bool) -> Result<()> {
        for entry in root.entries(root.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            let name = entry.as_os_str().to_str().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    format!("non-UTF-8 {} control entry", P::LABEL),
                )
            })?;
            let permitted = if top {
                name == HEAD || name.parse::<OperationId>().is_ok()
            } else {
                matches!(name, PLAN | COMPLETE)
            };
            if !permitted {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    format!("unknown or unfinished {} control write", P::LABEL),
                ));
            }
        }
        Ok(())
    }
}

fn optional<T: for<'de> Deserialize<'de>>(root: &DiskRoot, name: &str) -> Result<Option<T>> {
    root.open_optional_read_regular(&root.path().join(name))?
        .map(|file| Ok(decode_strict(&read_file_bytes(file, MAX_METADATA)?)?))
        .transpose()
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/engine/activation_plan_store_tests.rs"]
mod tests;
