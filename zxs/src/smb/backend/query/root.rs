//! Pure selection for the original open of the single ZXS root. No reader
//! of the native bridge, business grant, record encoder, or socket here.
use super::{DirectoryPage, MAX_RECORDS, PageStop, Pattern, PatternError, QueryObject, RecordSpace};
use crate::{display, smb::{backend::Held, wire::query_fields::{DirectoryClass, DirectoryQuery, Scan}}};
use std::{io, sync::{Arc, Weak}};

pub(in crate::smb::backend) mod records;

// Catalog enrolls at most 64 roots, one of which is SYSTEM. The namespace
// can contain the selected ordinary children and at most one recycle child.
const MAX_CHILDREN: usize = 63;
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;

pub(in crate::smb::backend) struct Child {
    pub(in crate::smb::backend) name: String,
    /// The original logical folder number from the authorized projection,
    /// not a Core FILE ID, an SMB2 OPEN ID, an inode, or a storage locator.
    pub(in crate::smb::backend) number: u64,
}

/// Only a real original namespace OPEN may implement this in production.
/// Return the complete authorized projection from one current Core cut,
/// in exact UTF-8 byte order. Do not page it in advance, probe host names
/// or media, or report an unavailable/denied source as an empty projection.
/// The actual opened root, active subject and installed plan are all checked
/// by the source; this is not a permission check over caller-provided rows.
pub(in crate::smb::backend) trait OriginalNamespace {
    fn root_projection(&self) -> io::Result<Vec<Child>>;
}

struct Position {
    pattern: Pattern,
    after: Option<String>,
}

// This identity has no numeric constructor or relationship to a wire ID.
struct Proposal;

/// The only committed position and outstanding-page indicator of the OPEN.
/// A result owns the strong marker; this weak reference cannot keep an
/// abandoned result, OPEN, or Core lease alive. No lock or second cursor.
#[derive(Default)]
pub(in crate::smb::backend) struct Search {
    position: Option<Position>,
    pending: Weak<Proposal>,
}

#[derive(Debug)]
pub(in crate::smb::backend) enum Fault {
    NotNamespace,
    ListDenied,
    PendingReply,
    Pattern(PatternError),
    Budget(PageStop),
    Original(io::Error),
    InvalidProjection,
    Resources,
}

pub(in crate::smb::backend) struct Pick {
    pub(in crate::smb::backend) child: Child,
    pub(in crate::smb::backend) space: RecordSpace,
}

pub(in crate::smb::backend) enum Selection {
    Rows { entries: Vec<Pick>, bytes: usize, exhausted: bool },
    FirstOverflow { child: Child, name_space: usize },
    NoMatch { first_query: bool },
}

/// An owned response proposal. It may survive the command's short borrow of
/// the real OPEN, but cannot keep it open or select another OPEN by wire ID.
/// Only the reply owner can later pair it with the same live OPEN, original
/// request and full protected write. Dropping it never moves the cursor.
#[must_use]
pub(in crate::smb::backend) struct RootPage {
    stamp: Arc<Proposal>,
    replacement: Option<Pattern>,
    restart: bool,
    class: DirectoryClass,
    output: Selection,
}

#[derive(Clone, Copy)]
pub(in crate::smb) enum EndOfRoot { NoInitialMatch, Exhausted }

/// Can only come from an actual authorized projection with no matches.
/// A loss of authority, short buffer or underlying I/O error is not this.
/// Keep the same uncommitted pattern/position available for the eventual
/// write owner; there is no independently usable cursor acknowledgement.
#[must_use]
pub(in crate::smb) struct RootScanEnd {
    kind: EndOfRoot,
    _proposal: RootPage,
}

impl RootScanEnd {
    pub(in crate::smb) fn kind(&self) -> EndOfRoot { self.kind }
}

impl RootPage {
    pub(in crate::smb::backend) fn seal_end(self) -> Result<RootScanEnd, Self> {
        let kind = match self.output() {
            Selection::NoMatch { first_query: true } => EndOfRoot::NoInitialMatch,
            Selection::NoMatch { first_query: false } => EndOfRoot::Exhausted,
            _ => return Err(self),
        };
        Ok(RootScanEnd { kind, _proposal: self })
    }

    pub(in crate::smb::backend) fn output(&self) -> &Selection { &self.output }

    #[cfg(test)]
    pub(in crate::smb::backend) fn written_for_fixture<H>(self, open: &mut Held<'_, H>) -> bool {
        // Test-only stand-in for a completely written original reply.
        // No fixture may commit the result against a different OPEN.
        let search = &mut open.root_search;
        if !search.pending.ptr_eq(&Arc::downgrade(&self.stamp)) {
            return false;
        }
        search.pending = Weak::new();
        let Self { replacement, restart, output, .. } = self;
        let position = &mut search.position;
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
        true
    }
}

impl<H: OriginalNamespace> Held<'_, H> {
    pub(in crate::smb::backend) fn prepare_root_query(
        &mut self, query: &DirectoryQuery<'_>, response_room: usize,
    ) -> Result<RootPage, Fault> {
        // The dispatcher must first resolve both wire ID words in its actual
        // current session/tree. Here only that original bound OPEN is used.
        if self.query_object() != QueryObject::LogicalRoot { return Err(Fault::NotNamespace); }
        if self.grant.0 & FILE_LIST_DIRECTORY == 0 { return Err(Fault::ListDenied); }
        if self.root_search.pending.strong_count() != 0 { return Err(Fault::PendingReply); }
        let mut budget = DirectoryPage::start(QueryObject::LogicalRoot, query, response_room)
            .map_err(Fault::Budget)?;
        let first_query = self.root_search.position.is_none();
        let restart = first_query || query.scan != Scan::Continue;
        let mut replacement = if restart { Pattern::from_query(query).map_err(Fault::Pattern)? } else { None };
        if first_query && replacement.is_none() { replacement = Some(Pattern::All); }
        let pattern = replacement.as_ref().or_else(|| self.root_search.position.as_ref().map(|p| &p.pattern))
            .expect("first query has a default pattern");
        // FSA 2.1.5.6.3: absent replacement on a rewind keeps the old
        // pattern; an ordinary continuation ignores even a supplied pattern.
        let after = if restart { None } else { self.root_search.position.as_ref().and_then(|p| p.after.as_deref()) };
        let rows = self.original.root_projection().map_err(Fault::Original)?;
        if rows.len() > MAX_CHILDREN || rows.windows(2).any(|r| r[0].name.as_bytes() >= r[1].name.as_bytes()) {
            return Err(Fault::InvalidProjection);
        }
        let mut seen = 0u64;
        for r in &rows {
            // Number 1 belongs to the virtual root itself, not to a child.
            // Root rows are enrolled ordinary folders (and, when installed,
            // one recycle folder). Neither round IDs nor host inodes fit here.
            if !(2..=(MAX_CHILDREN as u64 + 1)).contains(&r.number)
                || display::component(r.name.as_bytes(), display::MAX).is_err()
            {
                return Err(Fault::InvalidProjection);
            }
            let bit = 1u64 << (r.number - 2);
            if seen & bit != 0 { return Err(Fault::InvalidProjection); }
            seen |= bit;
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
        // Nothing failed after claiming this one seat. It remains occupied
        // even when the response body is separated from the OPEN borrow.
        let stamp = Arc::new(Proposal);
        self.root_search.pending = Arc::downgrade(&stamp);
        Ok(RootPage { stamp, replacement, restart, class: query.class, output })
    }
}
