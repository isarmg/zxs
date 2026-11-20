//! Pure selection for the original open of the single ZXS root. No reader
//! of the native bridge, business grant, record encoder, or socket here.
use super::{DirectoryPage, MAX_RECORDS, PageStop, Pattern, PatternError, QueryObject, RecordSpace};
use crate::{display, smb::{backend::Held, wire::query_fields::{DirectoryQuery, Scan}}};
use std::io;

// Catalog enrolls at most 64 roots, one of which is SYSTEM. The namespace
// can contain the selected ordinary children and at most one recycle child.
const MAX_CHILDREN: usize = 63;
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;

pub(in crate::smb::backend) struct Child<M> {
    pub(in crate::smb::backend) name: String,
    pub(in crate::smb::backend) metadata: M,
}

/// Only a real original namespace OPEN may implement this in production.
/// Return the complete authorized projection from one current Core cut,
/// in exact UTF-8 byte order. Do not page it in advance, probe host names
/// or media, or report an unavailable/denied source as an empty projection.
/// The actual opened root, active subject and installed plan are all checked
/// by the source; this is not a permission check over caller-provided rows.
pub(in crate::smb::backend) trait OriginalNamespace {
    type Metadata;
    fn root_projection(&self) -> io::Result<Vec<Child<Self::Metadata>>>;
}

pub(in crate::smb::backend) struct Position {
    pattern: Pattern,
    after: Option<String>,
}

#[derive(Debug)]
pub(in crate::smb::backend) enum Fault {
    NotNamespace,
    ListDenied,
    Pattern(PatternError),
    Budget(PageStop),
    Original(io::Error),
    InvalidProjection,
    Resources,
}

pub(in crate::smb::backend) struct Pick<M> {
    pub(in crate::smb::backend) child: Child<M>,
    pub(in crate::smb::backend) space: RecordSpace,
}

pub(in crate::smb::backend) enum Selection<M> {
    Rows { entries: Vec<Pick<M>>, bytes: usize, exhausted: bool },
    FirstOverflow { child: Child<M>, name_space: usize },
    NoMatch { first_query: bool },
}

/// One proposal exclusively borrows the original OPEN's only search position.
/// The future reply owner must check its original request and current scope,
/// then commit only after writing the exact protected reply on that stream.
/// Dropping this proposal, including after a failed write, leaves no change.
/// There is deliberately no production acknowledgement method yet.
#[must_use]
pub(in crate::smb::backend) struct RootPage<'open, M> {
    position: &'open mut Option<Position>,
    replacement: Option<Pattern>,
    restart: bool,
    output: Selection<M>,
}

impl<M> RootPage<'_, M> {
    pub(in crate::smb::backend) fn output(&self) -> &Selection<M> { &self.output }

    #[cfg(test)]
    pub(in crate::smb::backend) fn written_for_fixture(self) {
        // A test explicitly models submission; production must not use this
        // to move the one original cursor independently of the reply owner.
        let Self { position, replacement, restart, output } = self;
        let after = match output {
            Selection::Rows { mut entries, .. } => entries.pop().map(|e| e.child.name),
            _ => None, // Neither a partial name nor no match consumes a row.
        };
        if let Some(p) = position.as_mut() {
            if let Some(pattern) = replacement { p.pattern = pattern; }
            if restart || after.is_some() { p.after = after; }
        } else if let Some(pattern) = replacement {
            *position = Some(Position { pattern, after });
        }
    }
}

impl<H: OriginalNamespace> Held<'_, H> {
    pub(in crate::smb::backend) fn prepare_root_query(
        &mut self, query: &DirectoryQuery<'_>, response_room: usize,
    ) -> Result<RootPage<'_, H::Metadata>, Fault> {
        // The dispatcher must first resolve both wire ID words in its actual
        // current session/tree. Here only that original bound OPEN is used.
        if self.query_object() != QueryObject::LogicalRoot { return Err(Fault::NotNamespace); }
        if self.grant.0 & FILE_LIST_DIRECTORY == 0 { return Err(Fault::ListDenied); }
        let mut budget = DirectoryPage::start(QueryObject::LogicalRoot, query, response_room)
            .map_err(Fault::Budget)?;
        let first_query = self.root_position.is_none();
        let restart = first_query || query.scan != Scan::Continue;
        let mut replacement = if restart { Pattern::from_query(query).map_err(Fault::Pattern)? } else { None };
        if first_query && replacement.is_none() { replacement = Some(Pattern::All); }
        let pattern = replacement.as_ref().or_else(|| self.root_position.as_ref().map(|p| &p.pattern))
            .expect("first query has a default pattern");
        // FSA 2.1.5.6.3: absent replacement on a rewind keeps the old
        // pattern; an ordinary continuation ignores even a supplied pattern.
        let after = if restart { None } else { self.root_position.as_ref().and_then(|p| p.after.as_deref()) };
        let rows = self.original.root_projection().map_err(Fault::Original)?;
        if rows.len() > MAX_CHILDREN
            || rows.iter().any(|r| display::component(r.name.as_bytes(), display::MAX).is_err())
            || rows.windows(2).any(|r| r[0].name.as_bytes() >= r[1].name.as_bytes())
        {
            return Err(Fault::InvalidProjection);
        }
        let mut entries = Vec::new();
        entries.try_reserve_exact(rows.len().min(if query.single { 1 } else { MAX_RECORDS }))
            .map_err(|_| Fault::Resources)?;
        let mut first_overflow = None;
        let mut exhausted = true;
        for child in rows {
            if after.is_some_and(|key| child.name.as_bytes() <= key.as_bytes()) || !pattern.matches_root(&child.name) {
                continue;
            }
            match budget.offer(&child.name) {
                Ok(space) => entries.push(Pick { child, space }),
                Err(PageStop::Full) => { exhausted = false; break; }
                Err(PageStop::FirstRecordOverflow { name_space }) => {
                    first_overflow = Some(Selection::FirstOverflow { child, name_space });
                    break;
                }
                Err(e) => return Err(Fault::Budget(e)),
            }
        }
        let output = if let Some(overflow) = first_overflow {
            overflow
        } else if entries.is_empty() {
            Selection::NoMatch { first_query }
        } else {
            Selection::Rows { entries, bytes: budget.bytes(), exhausted }
        };
        Ok(RootPage { position: &mut self.root_position, replacement, restart, output })
    }
}
