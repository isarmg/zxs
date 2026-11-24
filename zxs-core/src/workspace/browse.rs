//! Existing-workspace custody for the explicitly selected Browse capability.
use super::Workspace;
use crate::{
    Result, invalid,
    r3::{Branch, Catalog, Detail, History, Id},
    session::BrowseSession,
};
use std::sync::Arc;
use zeroize::Zeroizing;

impl Workspace {
    /// Explicit login for metadata reads only. Native HTTPS/2 and exact
    /// workspace checks must pass before the secluded prompt is invoked.
    pub async fn connect_browse(
        &self,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
    ) -> Result<BrowseSession> {
        self.validate()?;
        BrowseSession::connect_bound(
            &self.principal,
            Arc::clone(&self.read_session_owner),
            spelling,
            account,
            prompt,
            || self.validate(),
        )
        .await
    }

    fn browse_guard<'a>(&'a self, session: &'a BrowseSession) -> impl Fn() -> Result<()> + 'a {
        move || {
            self.validate()?;
            if !session.belongs_to(&self.read_session_owner, &self.principal) {
                return Err(invalid("the Browse session needs its original workspace"));
            }
            Ok(())
        }
    }

    pub async fn browse_catalog(
        &self,
        session: &BrowseSession,
        after: Option<&Id>,
    ) -> Result<Catalog> {
        session.catalog(after, self.browse_guard(session)).await
    }

    /// Continue only at the caller's original detail revision. An "after"
    /// cursor without that revision is not an authorized request.
    pub async fn browse_detail(
        &self,
        session: &BrowseSession,
        container: &Id,
        revision: Option<u64>,
        after: Option<&Branch>,
    ) -> Result<Detail> {
        session
            .detail(container, revision, after, self.browse_guard(session))
            .await
    }

    /// Read the caller's original current head and, if chosen, its exact
    /// continuation ancestor. A changed head is never followed implicitly.
    pub async fn browse_history(
        &self,
        session: &BrowseSession,
        container: &Id,
        branch: &Branch,
        head: &Id,
        next: Option<&Id>,
    ) -> Result<History> {
        session
            .history(container, branch, head, next, self.browse_guard(session))
            .await
    }

    pub async fn browse_tags(
        &self,
        session: &BrowseSession,
        container: &Id,
    ) -> Result<Vec<String>> {
        session.tags(container, self.browse_guard(session)).await
    }
}
