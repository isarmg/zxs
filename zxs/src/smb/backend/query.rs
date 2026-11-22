//! Query input, root selection and one response's record budget. The only
//! source of rows is the original authorized object; no reply is sent here.
pub(super) mod root;
use super::{Held, Object};
use crate::{display, smb::wire::query_fields::DirectoryQuery};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueryObject {
    LogicalRoot,
    Directory,
    File,
}

impl<H> Held<H> {
    /// Get the object's shape from the real open, not from a search pattern,
    /// the SMB2_FILEID bytes, or a client's proposed attributes.
    pub(super) fn query_object(&self) -> QueryObject {
        QueryObject::from_original(self._slot.object)
    }
}

impl QueryObject {
    fn from_original(object: Object) -> Self {
        match object {
            Object::Namespace(_) => Self::LogicalRoot,
            Object::OrdinaryRoot(..) | Object::Directory(..) |
            Object::RecycleRoot(_) | Object::DeleteRound(..) => Self::Directory,
            Object::File(..) => Self::File,
        }
    }
}

/// A predicate on a single already-open directory, never a pathname. None
/// means that the client supplied no new pattern; in particular a continuing
/// scan must not discard its original pattern. No case folding or short-name
/// alias is introduced here.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Pattern {
    All,
    Current,
    Parent,
    Exact(String),
    Expression(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PatternError {
    BadName,
    TooLong,
    Resources,
}

impl Pattern {
    pub(super) fn from_query(query: &DirectoryQuery<'_>) -> Result<Option<Self>, PatternError> {
        let raw = query.pattern();
        if raw.is_empty() {
            return Ok(None);
        }
        let units = raw.len() / 2;
        if units > display::MAX {
            return Err(PatternError::TooLong);
        }
        let mut text = String::new();
        text.try_reserve_exact(3 * units).map_err(|_| PatternError::Resources)?;
        let words = raw.as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes);
        for scalar in char::decode_utf16(words) {
            text.push(scalar.map_err(|_| PatternError::BadName)?);
        }
        Ok(Some(match text.as_str() {
            "*" | "*.*" => Self::All,
            "." => Self::Current,
            ".." => Self::Parent,
            _ if text.chars().any(|c| matches!(c, '*' | '?' | '<' | '>' | '"')) => {
                if text.chars().any(|c| c.is_control() || matches!(c, '\\' | '/' | ':' | '|')) {
                    return Err(PatternError::BadName);
                }
                // These are MS-CIFS/MS-FSA expression tokens, not display
                // characters. Preserve the expression for the search layer;
                // parsing it neither runs the matcher nor walks a parent.
                Self::Expression(text)
            }
            _ => {
                display::component(text.as_bytes(), display::MAX).map_err(|_| {
                    if text.len() > display::MAX { PatternError::TooLong } else { PatternError::BadName }
                })?;
                Self::Exact(text)
            }
        }))
    }

    // This slice does not generate `.` / `..` pseudo-records for ordinary
    // folders. FSA §2.1.5.6.3's real-volume-root and wildcard-that-matches
    // dot exceptions let these planned patterns use actual direct children.
    pub(super) fn supports_ordinary(&self,nested:bool)->bool{
        match self{
            Self::Current|Self::Parent=>false,
            Self::All=>true,
            Self::Exact(_)=>!nested,
            Self::Expression(x)=>!nested||matches_expression(x,"."),
        }
    }

    pub(super) fn match_bounded(&self,name:&str,work:&mut usize)->std::io::Result<bool>{
        // A held query can select a sparse page from a real large directory.
        // Bound worst-case UTF-16 DP work in each actual SQL cut as well as
        // the independent Core row/response-record limits.
        const MAX_STATES:usize=2_000_000;
        if let Self::Expression(x)=self{
            let cost=x.encode_utf16().count()*name.encode_utf16().count();
            *work=work.checked_add(cost).filter(|n|*n<=MAX_STATES)
                .ok_or_else(||std::io::Error::new(std::io::ErrorKind::OutOfMemory,"ordinary directory pattern work budget"))?;
        }
        Ok(self.matches_root(name))
    }

    fn matches_root(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            // ZXS's virtual namespace starts at its share root. There is no
            // host parent or fabricated "."/".." member to enumerate.
            Self::Current | Self::Parent => false,
            Self::Exact(want) => want.as_str() == name,
            Self::Expression(want) => matches_expression(want, name),
        }
    }
}

// MS-FSA 2.1.4.4: run on UTF-16 code units with ZXS's exact case and no
// short-name aliases. Two bounded rows of suffix states, no backtracking.
// Both inputs were bounded by the request/display grammar.
fn matches_expression(pattern: &str, name: &str) -> bool {
    let mut expression = [0u16; display::MAX];
    let mut words = [0u16; display::MAX];
    let (plen, nlen) = (pattern.encode_utf16().count(), name.encode_utf16().count());
    if plen > expression.len() || nlen > words.len() || nlen == 0 { return false; }
    for (dst, unit) in expression.iter_mut().zip(pattern.encode_utf16()) { *dst = unit; }
    for (dst, unit) in words.iter_mut().zip(name.encode_utf16()) { *dst = unit; }
    const DOT: u16 = b'.' as u16;
    let last_dot = words[..nlen].iter().rposition(|&w| w == DOT);
    let mut suffix = [false; display::MAX + 1];
    suffix[nlen] = true;
    for &op in expression[..plen].iter().rev() {
        let mut current = [false; display::MAX + 1];
        current[nlen] = matches!(op, 0x2a | 0x3c | 0x3e | 0x22) && suffix[nlen];
        for n in (0..nlen).rev() {
            current[n] = match op {
                0x2a => suffix[n] || current[n + 1], // *
                0x3f => suffix[n + 1],              // ?
                0x3c => suffix[n] || (last_dot != Some(n) && current[n + 1]), // DOS_STAR
                0x3e => suffix[if words[n] == DOT { n } else { n + 1 }],     // DOS_QM
                0x22 => words[n] == DOT && suffix[n + 1],                   // DOS_DOT
                w => words[n] == w && suffix[n + 1],
            };
        }
        suffix = current;
    }
    suffix[0]
}

// Independent cap on records in one SMB reply. Core's own page/cursor and
// per-command scan-work limit remain separate from this output quota.
const MAX_RECORDS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PageStop {
    NotDirectory,
    ClientLengthMismatch,
    ResponseSpace,
    FirstRecordOverflow { name_space: usize },
    Full,
    InvalidRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RecordSpace {
    /// Offset from this query's response output buffer. Only the distance
    /// between admitted records contributes to a NextEntryOffset.
    pub(super) offset: usize,
    pub(super) name_bytes: usize,
}

/// Sizes the complete matching prefix, without accepting a later shorter
/// entry after an earlier entry failed to fit. An empty plan is not EOF.
/// Only the real directory adapter can advance scan state: it must recheck
/// the original tree and FILE_LIST_DIRECTORY, keep the next unreturned
/// matching row, and use true source exhaustion for the end statuses.
#[derive(Debug)]
pub(super) struct DirectoryPage {
    client: usize,
    room: usize,
    fixed: usize,
    limit: usize,
    bytes: usize,
    records: usize,
    stopped: Option<PageStop>,
}

impl DirectoryPage {
    /// response_room is the remaining payload space after reserving SMB
    /// headers and compound padding. It is not the advertised output size.
    pub(super) fn start(
        object: QueryObject,
        query: &DirectoryQuery<'_>,
        response_room: usize,
    ) -> Result<Self, PageStop> {
        if object == QueryObject::File {
            return Err(PageStop::NotDirectory);
        }
        let fixed = query.class.fixed_bytes();
        let client = usize::try_from(query.output_bytes).map_err(|_| PageStop::ResponseSpace)?;
        if client < fixed {
            return Err(PageStop::ClientLengthMismatch);
        }
        if response_room < fixed {
            return Err(PageStop::ResponseSpace);
        }
        Ok(Self {
            client,
            room: response_room,
            fixed,
            limit: if query.single { 1 } else { MAX_RECORDS },
            bytes: 0,
            records: 0,
            stopped: None,
        })
    }

    pub(super) fn records(&self) -> usize {
        self.records
    }

    /// No trailing inter-record padding after the last complete entry.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The name must come from the authoritative logical listing, never a
    /// host dentry. This only sizes a name; it does not return a record or
    /// consume any cursor, including in the first-record overflow case.
    pub(super) fn offer(&mut self, name: &str) -> Result<RecordSpace, PageStop> {
        if let Some(stop) = self.stopped {
            return Err(stop);
        }
        let result = self.fit(name);
        match result {
            Ok(space) => {
                self.bytes = space.offset + self.fixed + space.name_bytes;
                self.records += 1;
            }
            Err(stop) => self.stopped = Some(stop),
        }
        result
    }

    fn fit(&self, name: &str) -> Result<RecordSpace, PageStop> {
        if self.records == self.limit {
            return Err(PageStop::Full);
        }
        if name.is_empty() || name.len() > display::MAX {
            return Err(PageStop::InvalidRecord);
        }
        // 16 records at most; fixed <= 104, name <= 510 UTF-16 bytes.
        let offset = (self.bytes + 7) & !7;
        let name_bytes = name.encode_utf16().count() * 2;
        let end = offset + self.fixed + name_bytes;
        if end > self.client.min(self.room) {
            return Err(if self.records != 0 {
                PageStop::Full
            } else if self.room < self.client {
                PageStop::ResponseSpace
            } else {
                // MS-FSA §2.1.5.6.3: first name may need partial output
                // with STATUS_BUFFER_OVERFLOW. Do not split a surrogate pair;
                // this code only computes room and never generates partial data.
                PageStop::FirstRecordOverflow { name_space: (self.client - self.fixed) & !1 }
            });
        }
        Ok(RecordSpace { offset, name_bytes })
    }
}

#[cfg(test)]
#[path = "../../test/smb/backend_query.rs"]
mod tests;
