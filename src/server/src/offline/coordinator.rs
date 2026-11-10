//! One root-side owner for the stopped-service maintenance coordinators.
//! Domain plans retain their formats, checks, confirmations and worker actions.
use super::{Result, managed_layout as layout, private_io::BundleDestination};
use data_serialization::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{marker::PhantomData, path::Path};

pub(super) trait Plan {
    const ROOT_REQUIRED: &'static str;
    const CONTEXT_CHANGED: &'static str;
    const PENDING: &'static str;
    fn context(&self) -> &layout::Context;
    fn check_storage(&self, _layout: &layout::Layout) -> Result<()> {
        Ok(())
    }
}

pub(super) struct Root<P> {
    pub(super) layout: layout::Layout,
    pub(super) context: layout::Context,
    pub(super) host: BundleDestination,
    plan: PhantomData<fn() -> P>,
}
impl<P: Plan> Root<P> {
    pub(super) fn open(user: String) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(P::ROOT_REQUIRED.into());
        }
        let host = BundleDestination::open(Path::new("/zxs"))?;
        let layout = layout::Layout::open(user)?;
        let context = layout.context(host.identity());
        Ok(Self {
            layout,
            context,
            host,
            plan: PhantomData,
        })
    }
    pub(super) fn check(&self, plan: &P) -> Result<()> {
        self.host.recheck()?;
        self.layout.recheck()?;
        if &self.context != plan.context() {
            return Err(P::CONTEXT_CHANGED.into());
        }
        plan.check_storage(&self.layout)
    }
    /// Move the admitted context out and release root-side handles before the
    /// service-UID child acquires its own lease and rechecks that exact context.
    pub(super) fn into_context(self) -> layout::Context {
        self.context
    }
}

/// These original plans use their exact digest as the retained directory key.
/// Operation-ID-keyed chains and first-storage recovery keep their own readers.
pub(super) trait RetainedPlan: Plan + Serialize + DeserializeOwned {
    const RECORD_PREFIX: &'static str;
    const DIGEST_REQUIRED: &'static str;
    const MISSING: &'static str;
    const DIGEST_CHANGED: &'static str;
}
impl<P: RetainedPlan> Root<P> {
    pub(super) fn retained(&self, expected: &str) -> Result<P> {
        request_digest::<P>(expected)?;
        let bytes = self
            .host
            .read_operation(
                &format!("{}{expected}", P::RECORD_PREFIX),
                "plan.json",
                64 * 1024,
            )?
            .ok_or(P::MISSING)?;
        let plan: P = json_codec::from_slice(&bytes)?;
        if digest(&plan)? != expected {
            return Err(P::DIGEST_CHANGED.into());
        }
        self.check(&plan)?;
        Ok(plan)
    }
}
pub(super) fn request_digest<P: RetainedPlan>(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(P::DIGEST_REQUIRED.into());
    }
    Ok(())
}
pub(super) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(
        value,
    )?)))
}

type Cause = Box<dyn std::error::Error + Send + Sync>;
#[derive(Debug)]
struct Pending {
    message: &'static str,
    source: Cause,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.message, self.source)
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
pub(super) fn pending<P: Plan>(source: Cause) -> Cause {
    Box::new(Pending {
        message: P::PENDING,
        source,
    })
}

#[cfg(test)]
#[path = "../../../../test/server/unit/offline/coordinator_tests.rs"]
mod tests;
