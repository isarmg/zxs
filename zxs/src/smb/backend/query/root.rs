//! The one owned search position and response proposal per actual folder
//! OPEN, whether virtual ZXS root or an authorized ordinary directory.
use super::{DirectoryPage, MAX_RECORDS, PageStop, Pattern, PatternError, QueryObject, RecordSpace};
use crate::{display, smb::{backend::Held, wire::{query_fields::{DirectoryClass, DirectoryQuery, Scan},query_reply::DirectoryReply}}};
use std::{io, sync::{Arc, Weak}};

pub(in crate::smb::backend) mod records;

// Catalog enrolls at most 64 roots, one of which is SYSTEM. The namespace
// can contain the selected ordinary children and at most one recycle child.
const MAX_CHILDREN: usize = 63;
const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;

pub(in crate::smb::backend) struct Child {
    pub(in crate::smb::backend) name: String,
    /// Original logical volume-root number or FILE's established 64-bit
    /// number (MS-FSCC 2.1.9). Never a Core FileId, SMB2 OPEN or host inode.
    pub(in crate::smb::backend) number: u64,
    /// Actual selected body size for a FILE. None denotes a directory,
    /// not a size of zero or a held body readable under this OPEN.
    pub(in crate::smb::backend) eof: Option<u64>,
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
    UnsupportedPattern,
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
    pub(in crate::smb) fn pending(&self)->RootPendingToken<'_>{self._proposal.pending()}
    pub(in crate::smb) fn into_effect(self)->RootCursorEffect{self._proposal.into_effect()}
}

#[derive(Debug)]
pub(in crate::smb::backend) enum ReplyFault { Original, Resources, FirstRecordOverflow }

impl RootPage {
    pub(in crate::smb::backend) fn into_reply(self)->Result<DirectoryReply,ReplyFault>{
        match self.output(){
            Selection::Rows{..}=>{
                let records=self.encode_complete().map_err(|fault|match fault{
                    records::EncodeFault::Resources=>ReplyFault::Resources,_=>ReplyFault::Original,
                })?;
                DirectoryReply::complete_root(records).map_err(|_|ReplyFault::Original)
            }
            Selection::NoMatch{..}=>Ok(DirectoryReply::no_matching_root(
                self.seal_end().map_err(|_|ReplyFault::Original)?)),
            // No partial data and no acknowledgement of an unreturned name.
            Selection::FirstOverflow{..}=>Err(ReplyFault::FirstRecordOverflow),
        }
    }
    pub(in crate::smb::backend) fn seal_end(self) -> Result<RootScanEnd, Self> {
        let kind = match self.output() {
            Selection::NoMatch { first_query: true } => EndOfRoot::NoInitialMatch,
            Selection::NoMatch { first_query: false } => EndOfRoot::Exhausted,
            _ => return Err(self),
        };
        Ok(RootScanEnd { kind, _proposal: self })
    }

    pub(in crate::smb::backend) fn output(&self) -> &Selection { &self.output }

    pub(in crate::smb::backend) fn pending(&self)->RootPendingToken<'_>{RootPendingToken(&self.stamp)}

    pub(in crate::smb::backend) fn into_effect(self)->RootCursorEffect{
        let Self{stamp,replacement,restart,output,..}=self;
        let after=match output{
            Selection::Rows{mut entries,..}=>entries.pop().map(|e|e.child.name),
            _=>None,
        };
        RootCursorEffect{stamp,replacement,restart,after}
    }

}

/// Borrow only this prepared proposal marker, never the OPEN or Core. It
/// cannot be created from packet IDs or a decoded cursor.
pub(in crate::smb) struct RootPendingToken<'a>(&'a Arc<Proposal>);
impl RootPendingToken<'_>{
    pub(in crate::smb::backend) fn belongs_to<H>(&self,open:&Held<H>)->bool{
        open.root_search.pending.ptr_eq(&Arc::downgrade(self.0))
    }
}

/// The only proposal eligible to advance a root search. The backend can
/// apply it only on the original open, after the real sender's sealed receipt.
pub(in crate::smb) struct RootCursorEffect{
    stamp:Arc<Proposal>,replacement:Option<Pattern>,restart:bool,after:Option<String>,
}
impl RootCursorEffect{
    pub(in crate::smb::backend) fn apply<H>(self,open:&mut Held<H>)->bool{
        if !RootPendingToken(&self.stamp).belongs_to(open){return false;}
        let Self{replacement,restart,after,..}=self;
        let search=&mut open.root_search;
        search.pending=Weak::new();
        let position=&mut search.position;
        if let Some(p)=position.as_mut(){
            if let Some(pattern)=replacement{p.pattern=pattern;}
            if restart||after.is_some(){p.after=after;}
        }else if let Some(pattern)=replacement{
            *position=Some(Position{pattern,after});
        }
        true
    }
}


impl<H> Held<H>{
    pub(in crate::smb::backend) fn prepare_root_query_with<P>(
        &mut self,query:&DirectoryQuery<'_>,response_room:usize,
        original:impl FnOnce(&H)->io::Result<(Vec<Child>,P)>,
    )->Result<(RootPage,P),Fault>{
        self.prepare_query_with(QueryObject::LogicalRoot,query,response_room,
            |held,pattern,after,_limit|{
                let (mut rows,proof)=original(held).map_err(Fault::Original)?;
                if rows.len()>MAX_CHILDREN || rows.windows(2).any(|r|r[0].name.as_bytes()>=r[1].name.as_bytes()){
                    return Err(Fault::InvalidProjection);
                }
                let mut seen=0u64;
                for r in &rows{
                    // Number 1 is the virtual root. These are enrolled folders,
                    // not a DirectoryId, FILE size, round ID or host inode.
                    if !(2..=(MAX_CHILDREN as u64 + 1)).contains(&r.number)||r.eof.is_some()||
                        display::component(r.name.as_bytes(),display::MAX).is_err(){
                        return Err(Fault::InvalidProjection);
                    }
                    let bit=1u64<<(r.number-2);
                    if seen&bit!=0{return Err(Fault::InvalidProjection);}
                    seen|=bit;
                }
                rows.retain(|r|after.is_none_or(|key|r.name.as_bytes()>key.as_bytes())&&pattern.matches_root(&r.name));
                Ok((rows,true,proof))
            })
    }

    /// Only the actual ordinary-directory OPEN calls this with a bounded,
    /// authorized Core selection. No skipped/unfit name commits a cursor.
    pub(in crate::smb::backend) fn prepare_ordinary_query_with<P>(
        &mut self,query:&DirectoryQuery<'_>,response_room:usize,
        original:impl FnOnce(&H,&Pattern,Option<&str>,u8)->Result<(Vec<Child>,P),Fault>,
    )->Result<(RootPage,P),Fault>{
        self.prepare_query_with(QueryObject::Directory,query,response_room,|held,pattern,after,limit|{
            let (rows,proof)=original(held,pattern,after,limit)?;
            if rows.len()>usize::from(limit)||
               rows.windows(2).any(|r|r[0].name.as_bytes()>=r[1].name.as_bytes())||
               rows.first().is_some_and(|r|after.is_some_and(|n|r.name.as_bytes()<=n.as_bytes()))||
               rows.iter().any(|r|display::component(r.name.as_bytes(),display::MAX).is_err()){
                return Err(Fault::InvalidProjection);
            }
            let exhausted=rows.len()<usize::from(limit);
            Ok((rows,exhausted,proof))
        })
    }

    fn prepare_query_with<P>(
        &mut self,object:QueryObject,query:&DirectoryQuery<'_>,response_room:usize,
        original:impl FnOnce(&H,&Pattern,Option<&str>,u8)->Result<(Vec<Child>,bool,P),Fault>,
    )->Result<(RootPage,P),Fault>{
        // Caller already resolved the actual session, tree and both OPEN
        // words. Here an independent granted access and pending slot gate
        // happens before any business projection or pattern selection.
        if self.query_object()!=object{return Err(Fault::NotNamespace);}
        if self.grant.0&FILE_LIST_DIRECTORY==0{return Err(Fault::ListDenied);}
        if self.root_search.pending.strong_count()!=0{return Err(Fault::PendingReply);}
        let mut budget=DirectoryPage::start(object,query,response_room).map_err(Fault::Budget)?;
        let limit=if query.single{1}else{MAX_RECORDS as u8};
        let first_query=self.root_search.position.is_none();
        let restart=first_query||query.scan!=Scan::Continue;
        let mut replacement=if restart{Pattern::from_query(query).map_err(Fault::Pattern)?}else{None};
        if first_query&&replacement.is_none(){replacement=Some(Pattern::All);}
        let pattern=replacement.as_ref().or_else(||self.root_search.position.as_ref().map(|p|&p.pattern))
            .ok_or(Fault::InvalidProjection)?;
        // FSA §2.1.5.6.3: a rewind without a new pattern keeps the old one.
        // A continuation ignores even a supplied replacement pattern.
        let after=if restart{None}else{self.root_search.position.as_ref().and_then(|p|p.after.as_deref())};
        let (rows,mut exhausted,proof)=original(&self.original,pattern,after,limit)?;
        let mut entries=Vec::new();
        entries.try_reserve_exact(rows.len().min(usize::from(limit))).map_err(|_|Fault::Resources)?;
        let mut first_overflow=None;
        for child in rows{
            match budget.offer(&child.name){
                Ok(space)=>entries.push(Pick{child,space}),
                Err(PageStop::Full)=>{exhausted=false;break;},
                Err(PageStop::FirstRecordOverflow{name_space})=>{
                    first_overflow=Some(Selection::FirstOverflow{child,name_space});
                    break;
                }
                Err(e)=>return Err(Fault::Budget(e)),
            }
        }
        let output=if let Some(overflow)=first_overflow{overflow}
            else if entries.is_empty()&&exhausted{Selection::NoMatch{first_query}}
            else if entries.is_empty(){return Err(Fault::InvalidProjection);}
            else{Selection::Rows{entries,bytes:budget.bytes(),exhausted}};
        let stamp=Arc::new(Proposal);
        self.root_search.pending=Arc::downgrade(&stamp);
        Ok((RootPage{stamp,replacement,restart,class:query.class,output},proof))
    }
}

#[cfg(test)]
include!("../../../../tests/unit/attached/smb/backend/query/root/items.rs");
