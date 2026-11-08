use crate::{Result, print};
use data_serialization::Serialize;
use std::path::Path;
use storage_engine::disk_authority::{
    Digest, DiskSelection, Error as DiskError, Identity, IndependentDisk, Token,
};

#[derive(Serialize)]
#[serialization(rename_all = "snake_case")]
enum State {
    NoPublishedContent,
    CompleteSuccess,
    ContentIssues,
}

#[derive(Serialize)]
#[serialization(tag = "status", rename_all = "snake_case")]
enum Content {
    Verified,
    TemporarilyUnavailable,
    Corrupt,
}

#[derive(Serialize)]
struct Copy {
    owner: Identity,
    history_id: Token,
    file_id: Token,
    commit: Digest,
    body: Digest,
    expected_bytes: u64,
    content: Content,
}

#[derive(Serialize)]
pub(crate) struct Report {
    format: &'static str,
    scope: &'static str,
    state: State,
    identity: Identity,
    checkpoint: Option<Digest>,
    sequence: u64,
    copies: Vec<Copy>,
}

pub(crate) fn collect(selection: &DiskSelection) -> std::result::Result<Report, DiskError> {
    let disk = IndependentDisk::open(selection)?;
    let mut snapshot = disk.content_snapshot()?;
    let versions: Vec<_> = snapshot.versions().cloned().collect();
    let mut copies = Vec::with_capacity(versions.len());
    for copy in &versions {
        let version = copy.version();
        let content = match snapshot.read_content(version.commit()) {
            Ok(_) => Content::Verified,
            Err(DiskError::ContentUnavailable(_)) => Content::TemporarilyUnavailable,
            Err(DiskError::Corrupt(_)) => Content::Corrupt,
            Err(error) => return Err(error),
        };
        copies.push(Copy {
            owner: version.owner().clone(),
            history_id: version.history_id().clone(),
            file_id: version.file_id().clone(),
            commit: version.commit(),
            body: version.body(),
            expected_bytes: copy.expected_bytes(),
            content,
        });
    }
    snapshot.recheck()?;
    let healthy = copies
        .iter()
        .all(|copy| matches!(copy.content, Content::Verified));
    let state = if copies.is_empty() {
        State::NoPublishedContent
    } else if healthy {
        State::CompleteSuccess
    } else {
        State::ContentIssues
    };
    Ok(Report {
        format: "independent-disk-check",
        scope: "local-retained-published-content",
        state,
        identity: snapshot.identity().clone(),
        checkpoint: snapshot.checkpoint(),
        sequence: snapshot.sequence(),
        copies,
    })
}

impl Report {
    pub(crate) fn healthy(&self) -> bool {
        !matches!(self.state, State::ContentIssues)
    }
}

pub(super) fn run(selection: &Path) -> Result<()> {
    let report = collect(&super::read::<DiskSelection>(selection)?)?;
    let healthy = report.healthy();
    print(report)?;
    if healthy {
        Ok(())
    } else {
        Err("independent disk check found missing or corrupt retained content; no repair was attempted".into())
    }
}
