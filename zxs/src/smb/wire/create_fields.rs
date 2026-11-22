//! SMB2 CREATE field ownership for ZXS. No target lookup, access grant,
//! replay, lease, durable handle, or successful OPEN is created here.
use super::{bytes,message::Message,path_fields::FieldError,u16le,u32le,u64le};

const MAX_CONTEXTS:usize=16;
const DIRECTORY:u32=0x0000_0001;
const NON_DIRECTORY:u32=0x0000_0040;
const DELETE_ON_CLOSE:u32=0x0000_1000;
const DELETE_ACCESS:u32=0x0001_0000;
const UNBUFFERED:u32=0x0000_0008;
const RESERVE_OPFILTER:u32=0x0010_0000;
const NO_COMPRESSION:u32=0x0000_8000;
const SEQUENTIAL:u32=0x0000_0004;
const RANDOM:u32=0x0000_0800;
const KNOWN_OPTIONS:u32=0x00f3_ff7f; // MS-SMB2 §2.2.13, not the full NT CreateOptions set
const IGNORED_OPTIONS:u32=0x0083_0530; // the seven bits §2.2.13 tells a server to ignore
const DIRECTORY_OPTIONS:u32=DIRECTORY|0x0000_0002|DELETE_ON_CLOSE|0x0000_4000|0x0020_0000;
const REPLAY:u32=0x2000_0000;
const RESERVED_CONTEXT:[u8;16]=[0x93,0xad,0x25,0x50,0x9c,0xb4,0x11,0xe7,0xb4,0x23,0x83,0xde,0x96,0x8b,0xcd,0x7c];

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Error {
    Field(FieldError), Malformed, Unsupported, ContextLimit,
}
type Result<T> = std::result::Result<T,Error>;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Disposition { Supersede, Open, Create, OpenIf, Overwrite, OverwriteIf }
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum OplockRequest { None, LevelII, Exclusive, Batch, Lease }
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum MaximumAccessQuery { Unconditional, CompareChangeTime(u64) }
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Kind {
    MaximalAccess(MaximumAccessQuery), DiskId, LeaseRequest, DurableRequestV1, DurableRequestV2,
    DurableReconnect, HistoricalOpen, AppleServerQuery, Reserved, Unresolved,
}

#[derive(Clone,Copy)]
pub(in crate::smb) struct Context<'a> {
    name:&'a [u8],data:&'a [u8],kind:Kind,
}
impl<'a> Context<'a> {
    pub(in crate::smb) fn name(&self)->&'a [u8] {self.name}
    pub(in crate::smb) fn data(&self)->&'a [u8] {self.data}
    pub(in crate::smb) fn kind(&self)->Kind {self.kind}
}
struct Contexts<'a> { items:[Option<Context<'a>>;MAX_CONTEXTS], len:usize }

/// Client intent only. No transformation of DesiredAccess into GrantedAccess.
/// Only the real scoped OPEN can apply policy, options and response obligations.
pub(in crate::smb) struct CreateFields<'a> {
    name:&'a [u8], contexts:Contexts<'a>, replay:bool,
    disposition:Disposition,desired_access:u32,file_attributes:u32,share_access:u8,
    oplock:OplockRequest,impersonation_level:u32,requested_options:u32,effective_options:u32,
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum CurrentName<'a> { Root, Relative(&'a [u8]) }
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum SpecialOpen { Replay, Reconnect, Historical, Context, RootIsDirectory, RootOpenOnly, Unbuffered }
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) struct CurrentIntent<'a> {
    name:CurrentName<'a>,
    // An accepted CREATE would have to answer these with its real object
    // metadata. Neither a client timestamp nor an ID in a request is data.
    maximal_access:Option<MaximumAccessQuery>,disk_id:bool,
}
impl<'a> CurrentIntent<'a> {
    pub(in crate::smb) fn name(&self)->CurrentName<'a> {self.name}
    pub(in crate::smb) fn maximal_access(&self)->Option<MaximumAccessQuery> {self.maximal_access}
    pub(in crate::smb) fn disk_id(&self)->bool {self.disk_id}
}

fn field<T>(r:std::result::Result<T,super::Error>)->Result<T> {r.map_err(|_|Error::Malformed)}

impl<'a> Message<'a> {
    pub(in crate::smb) fn create_fields(&self)->Result<CreateFields<'a>> {
        let n=self.create_name_fields().map_err(Error::Field)?;
        if n.name.len()%2!=0 {return Err(Error::Malformed);}
        let p=self.raw;
        // §3.3.5.9 still checks NameOffset when NameLength is zero.
        // No empty name can be promoted to a root from the fixed header
        // or beyond this command. §2.2.13 requires aligned NameOffset.
        let name_at=usize::from(field(u16le(p,108))?);
        if name_at<120 || name_at%8!=0 || name_at>p.len() {return Err(Error::Malformed);}
        let disposition=match field(u32le(p,100))? {
            0=>Disposition::Supersede,1=>Disposition::Open,2=>Disposition::Create,
            3=>Disposition::OpenIf,4=>Disposition::Overwrite,5=>Disposition::OverwriteIf,
            _=>return Err(Error::Malformed),
        };
        // The name-area parser already proved the fixed portion exists.
        // The four impersonation encodings are request metadata, NOT login or delegation.
        let oplock=match p[67] {
            0=>OplockRequest::None,1=>OplockRequest::LevelII,8=>OplockRequest::Exclusive,
            9=>OplockRequest::Batch,0xff=>OplockRequest::Lease,_=>return Err(Error::Malformed),
        };
        let impersonation_level=field(u32le(p,68))?;
        let share=field(u32le(p,96))?;
        let requested_options=field(u32le(p,104))?;
        let desired_access=field(u32le(p,88))?;
        if impersonation_level>3 || share&!7!=0 || requested_options&!KNOWN_OPTIONS!=0 {
            return Err(Error::Malformed);
        }
        // An explicit requested DELETE bit is required by §2.2.13.
        // MAXIMUM_ALLOWED (or a generic mask) isn't that request and is
        // never expanded here. Even the bit cannot confer an actual grant.
        if requested_options&DELETE_ON_CLOSE!=0 && desired_access&DELETE_ACCESS==0 {
            return Err(Error::Malformed);
        }
        if requested_options&RESERVE_OPFILTER!=0 {return Err(Error::Unsupported);}
        let mut effective_options=requested_options&!IGNORED_OPTIONS;
        if effective_options&RANDOM!=0 {effective_options&=!SEQUENTIAL;}
        if effective_options&DIRECTORY!=0 {
            effective_options&=!NO_COMPRESSION;
            if effective_options&!DIRECTORY_OPTIONS!=0 ||
                !matches!(disposition,Disposition::Open|Disposition::Create|Disposition::OpenIf) {
                return Err(Error::Malformed);
            }
        }
        let contexts=Contexts::parse(n.contexts)?;
        Ok(CreateFields{name:n.name,contexts,replay:self.header.flags&REPLAY!=0,disposition,
            desired_access,file_attributes:field(u32le(p,92))?,
            share_access:share as u8,oplock,impersonation_level,requested_options,effective_options})
    }
}

impl<'a> CreateFields<'a> {
    pub(in crate::smb) fn disposition(&self)->Disposition {self.disposition}
    pub(in crate::smb) fn desired_access(&self)->u32 {self.desired_access}
    pub(in crate::smb) fn file_attributes(&self)->u32 {self.file_attributes}
    pub(in crate::smb) fn share_access(&self)->u8 {self.share_access}
    pub(in crate::smb) fn oplock(&self)->OplockRequest {self.oplock}
    pub(in crate::smb) fn impersonation_level(&self)->u32 {self.impersonation_level}
    pub(in crate::smb) fn requested_options(&self)->u32 {self.requested_options}
    pub(in crate::smb) fn effective_options(&self)->u32 {self.effective_options}
    pub(in crate::smb) fn contexts(&self)->impl Iterator<Item=Context<'a>>+'_ {
        self.contexts.items[..self.contexts.len].iter().flatten().copied()
    }

    /// Target shape for a current-namespace operation only. Open/Create
    /// modes, access masks and the exact authorized tree still need checking.
    /// Opaque semantics, historical targets and reconnects never become an
    /// ordinary relative or empty-name open.
    pub(in crate::smb) fn current_intent(&self)->std::result::Result<CurrentIntent<'a>,SpecialOpen> {
        if self.replay {return Err(SpecialOpen::Replay);}
        let mut maximal_access=None;let mut disk_id=false;
        let (mut reconnect,mut historical,mut unresolved)=(false,false,false);
        for part in self.contexts() {
            match part.kind() {
                Kind::DurableReconnect=>reconnect=true,
                Kind::HistoricalOpen=>historical=true,
                Kind::Unresolved=>unresolved=true,
                Kind::MaximalAccess(ask)=>maximal_access=Some(ask),
                Kind::DiskId=>disk_id=true,
                // Requests are not leases or durable handles. Decline the
                // capability under the respective rules and do not emit an
                // AAPL server response for its known query-only extension.
                Kind::LeaseRequest|Kind::DurableRequestV1|Kind::DurableRequestV2|Kind::AppleServerQuery|Kind::Reserved=>{},
            }
        }
        if reconnect {return Err(SpecialOpen::Reconnect);}
        if historical {return Err(SpecialOpen::Historical);}
        if unresolved {return Err(SpecialOpen::Context);}
        if self.effective_options&UNBUFFERED!=0 {return Err(SpecialOpen::Unbuffered);}
        let name=if self.name.is_empty() {
            if self.effective_options&NON_DIRECTORY!=0 {return Err(SpecialOpen::RootIsDirectory);}
            if self.disposition!=Disposition::Open || self.effective_options&DELETE_ON_CLOSE!=0 {
                return Err(SpecialOpen::RootOpenOnly);
            }
            CurrentName::Root
        } else {CurrentName::Relative(self.name)};
        Ok(CurrentIntent{name,maximal_access,disk_id})
    }
}

impl<'a> Contexts<'a> {
    fn parse(mut p:&'a [u8])->Result<Self> {
        let mut out=Self{items:[None;MAX_CONTEXTS],len:0};
        let (mut v1,mut v2,mut reconnect)=(false,false,false);
        let (mut maximum,mut disk_id)=(false,false);
        while !p.is_empty() {
            if out.len==MAX_CONTEXTS {return Err(Error::ContextLimit);}
            let next=field(u32le(p,0))? as usize;
            if next!=0 && (next<16 || !next.is_multiple_of(8) || next>=p.len()) {return Err(Error::Malformed);}
            let (q,rest)=p.split_at(if next==0 {p.len()} else {next});
            let name_at=usize::from(field(u16le(q,4))?);
            let name_len=usize::from(field(u16le(q,6))?);
            if name_at<16 || name_at%8!=0 || name_len<4 {return Err(Error::Malformed);}
            let name=field(bytes(q,name_at,name_len))?;
            let data_len=field(u32le(q,12))? as usize;
            let data=if data_len==0 {
                &q[0..0] // DataOffset MUST be ignored when no data is present.
            } else {
                let at=usize::from(field(u16le(q,10))?);
                if at<16 || at%8!=0 {return Err(Error::Malformed);}
                let data=field(bytes(q,at,data_len))?;
                if name_at<at+data.len() && at<name_at+name.len() {return Err(Error::Malformed);}
                data
            };
            let kind=match name {
                b"MxAc" if !maximum=>{
                    maximum=true;
                    Kind::MaximalAccess(match data.len() {
                        0=>MaximumAccessQuery::Unconditional,
                        8=>MaximumAccessQuery::CompareChangeTime(field(u64le(data,0))?),
                        _=>return Err(Error::Malformed),
                    })
                }
                b"QFid" if !disk_id && data.is_empty()=>{disk_id=true;Kind::DiskId}
                b"RqLs" if matches!(data.len(),32|52)=>Kind::LeaseRequest,
                b"DHnQ" if !v1 && data.len()==16=>{v1=true;Kind::DurableRequestV1}
                b"DH2Q" if !v2 && data.len()==32=>{v2=true;Kind::DurableRequestV2}
                b"DHnC" if data.len()==16=>{reconnect=true;Kind::DurableReconnect}
                b"DH2C" if data.len()==36=>{reconnect=true;Kind::DurableReconnect}
                b"TWrp" if data.len()==8=>Kind::HistoricalOpen,
                b"MxAc"|b"QFid"|b"TWrp"|b"RqLs"|b"DHnQ"|b"DH2Q"|b"DHnC"|b"DH2C"=>
                    return Err(Error::Malformed),
                // This target-independent Apple probe is recognized only in
                // its known 24-byte server-query form. Never swallow resolve-ID.
                b"AAPL" if data.len()==24 && data[..8]==[1,0,0,0,0,0,0,0]=>Kind::AppleServerQuery,
                value if value==RESERVED_CONTEXT.as_slice()=>Kind::Reserved, // MS-SMB2 §2.2.13.2 MUST ignore
                _=>Kind::Unresolved,
            };
            out.items[out.len]=Some(Context{name,data,kind});out.len+=1;p=rest;
        }
        // MS-SMB2 3.3.5.9.10 is independent of context order.
        if v2 && (v1 || reconnect) {return Err(Error::Malformed);}
        Ok(out)
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/create_fields.rs"]
mod tests;
