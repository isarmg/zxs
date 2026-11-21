//! Closed, small synchronous replies to an original paid, signed member.
//! No generic success body or caller-selected wire ID.
use super::{Error,Result,create_fields::{Disposition,MaximumAccessQuery},io_reply::{FLUSH,FileReply,WRITE},message::{Message,RELATED,SIGNED,Routing},network_probe::{IOCTL,NetworkProbe},
    query_fields::{CapacityFormat,FileClass,FsClass,InfoClass},set_info::{Change,FileSetRequest,RenameRequest,SET_INFO}};
use crate::smb::{
    backend::{CloseCompletion,PreparedDisposition,PreparedOrdinaryRename,PreparedRootFsAttributes,PreparedRootFsSize,PreparedRetainedRestore,tree_connect::QualifiedTreeConnect},
    connection::sessions::ordinary::{
        BrowsePart,ClosedOpen,DirectoryRefusal,ExitPlan,FsInfoRefusal,PendingAttributeTag,PendingFileAccess,PendingFileStandard,PendingInternal,PendingOpen,PendingRootAttributes,PendingRootSize,ProbePart,ProposedTreeId,
        FileActionPart,PendingRename,PendingWrite,ReadRefusal,RenameRefusal,VerifiedMember,WriteRefusal,
    },
};
use crate::system::reader::smb::runtime::CapacityUnits;
use std::num::NonZeroU64;

const ECHO:u16=13;
const TREE_CONNECT:u16=3;
pub(in crate::smb) const LOGOFF:u16=2;
pub(in crate::smb) const TREE_DISCONNECT:u16=4;
const CREATE:u16=5;
const CLOSE:u16=6;
pub(super) const QUERY_INFO:u16=16;
const EMPTY_ERROR:usize=72;
const TREE_OK:usize=80;
const CREATE_FIXED:usize=152; // 64-byte header plus 88 fixed CREATE bytes
const MAXIMAL_CONTEXT:usize=32;
const QUERY_DISK_CONTEXT:usize=56;
pub(in crate::smb) const MAX_SMALL_REPLY:usize=CREATE_FIXED+MAXIMAL_CONTEXT+QUERY_DISK_CONTEXT;

struct OpenContexts{maximum:Option<u32>,disk_id:Option<(u64,u64)>}
impl OpenContexts{
    fn length(&self)->usize{
        usize::from(self.maximum.is_some())*MAXIMAL_CONTEXT+usize::from(self.disk_id.is_some())*QUERY_DISK_CONTEXT
    }
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum TreeFailure {
    InvalidParameter=0xc000_000d,
    AccessDenied=0xc000_0022,
    InsufficientResources=0xc000_009a,
    BadNetworkName=0xc000_00cc,
    ServerUnavailable=0xc000_0466,
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum NetworkProbeFailure{
    InvalidParameter=0xc000_000d,
    NotSupported=0xc000_00bb,
    NetworkNameDeleted=0xc000_00c9,
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum RootFailure {
    InvalidParameter=0xc000_000d,
    AccessDenied=0xc000_0022,
    ObjectNameInvalid=0xc000_0033,
    ObjectNameNotFound=0xc000_0034,
    ObjectNameCollision=0xc000_0035,
    ObjectPathNotFound=0xc000_003a,
    SharingViolation=0xc000_0043,
    DeletePending=0xc000_0056,
    DiskFull=0xc000_007f,
    InsufficientResources=0xc000_009a,
    MediaWriteProtected=0xc000_00a2,
    FileIsDirectory=0xc000_00ba,
    NotSupported=0xc000_00bb,
    NetworkNameDeleted=0xc000_00c9,
    NotADirectory=0xc000_0103,
    NameTooLong=0xc000_0106,
    FileClosed=0xc000_0128,
    VolumeDismounted=0xc000_026e,
    ServerUnavailable=0xc000_0466,
    FileNotAvailable=0xc000_0467,
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum FsInfoFailure {
    InvalidInfoClass=0xc000_0003,InformationLengthMismatch=0xc000_0004,InvalidParameter=0xc000_000d,
    AccessDenied=0xc000_0022,Resources=0xc000_009a,MediaWriteProtected=0xc000_00a2,DeviceNotReady=0xc000_00a3,
    NotSupported=0xc000_00bb,NetworkNameDeleted=0xc000_00c9,FileClosed=0xc000_0128,ServerUnavailable=0xc000_0466,
    VolumeDismounted=0xc000_026e,FileNotAvailable=0xc000_0467,
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum WriteFailure {
    InformationLengthMismatch=0xc000_0004,InvalidParameter=0xc000_000d,AccessDenied=0xc000_0022,DiskFull=0xc000_007f,Resources=0xc000_009a,
    MediaWriteProtected=0xc000_00a2,NotSupported=0xc000_00bb,NetworkNameDeleted=0xc000_00c9,
    InternalError=0xc000_00e5,FileClosed=0xc000_0128,VolumeDismounted=0xc000_026e,
    ServerUnavailable=0xc000_0466,FileNotAvailable=0xc000_0467,
}

#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum RestoreFailure{
    InformationLengthMismatch=0xc000_0004,InvalidParameter=0xc000_000d,AccessDenied=0xc000_0022,
    ObjectNameInvalid=0xc000_0033,ObjectNameCollision=0xc000_0035,ObjectPathNotFound=0xc000_003a,
    SharingViolation=0xc000_0043,DeletePending=0xc000_0056,
    DiskFull=0xc000_007f,Resources=0xc000_009a,MediaWriteProtected=0xc000_00a2,NotSupported=0xc000_00bb,
    NetworkNameDeleted=0xc000_00c9,NameTooLong=0xc000_0106,FileClosed=0xc000_0128,
    VolumeDismounted=0xc000_026e,ServerUnavailable=0xc000_0466,FileNotAvailable=0xc000_0467,
}

pub(in crate::smb) struct UnsignedSmall {raw:[u8;MAX_SMALL_REPLY],len:usize,sid:NonZeroU64}
pub(in crate::smb) struct PayableSmall(UnsignedSmall);
pub(in crate::smb) struct SignedSmall(UnsignedSmall);

impl UnsignedSmall {
    pub(in crate::smb) fn smb_len(&self)->usize{self.len}

    pub(in crate::smb) fn echo(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,compound:bool,more:bool)
        ->Result<Self> {
        let m=member.message();
        // Ignore the client's Reserved word, and zero our own. Incoming
        // command padding was checked as part of that member's signature.
        let ok=m.raw.len()>=68 && m.raw[64..66]==4u16.to_le_bytes();
        let status=if ok {0} else {0xc000_000d};
        let len=if compound || !ok {EMPTY_ERROR} else {68};
        let mut draft=Self::from_member(member,sid,ECHO,status,len,compound,more)?;
        draft.raw[36..40].fill(0); // ECHO returns no Tree
        draft.raw[64]=if ok {4} else {9};
        Ok(draft)
    }

    pub(in crate::smb) fn tree_failure(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,why:TreeFailure)
        ->Result<Self> {
        let m=member.message();
        if m.header.routing!=(Routing::Sync{tree:0}) || m.header.flags&RELATED!=0 || m.raw.get(20..24)!=Some(&[0;4][..]) {
            return Err(Error::InvalidRequest);
        }
        let mut draft=Self::from_member(member,sid,TREE_CONNECT,why as u32,EMPTY_ERROR,false,false)?;
        // MS-SMB2 §§2.2.2/3.3.4.4: no ErrorData for these plain failures;
        // StructureSize=9, eight fixed bytes, zero context count/ByteCount.
        draft.raw[64]=9;
        Ok(draft)
    }

    pub(in crate::smb) fn network_probe_failure(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,
        why:NetworkProbeFailure)->Result<Self>{
        use NetworkProbeFailure::{InvalidParameter,NetworkNameDeleted,NotSupported};
        let probe=member.message().network_interface_probe().ok_or(Error::InvalidRequest)?;
        let allowed=match (probe,why){
            (_,NetworkNameDeleted)|(NetworkProbe::NotFsctl,NotSupported)|
            (NetworkProbe::InvalidFileId|NetworkProbe::NoHandle{..},InvalidParameter)=>true,
            (NetworkProbe::NoHandle{max_output},NotSupported)=>max_output<=member.max_transact(),
            _=>false,
        };
        if member.ordinal()!=0||!allowed{return Err(Error::InvalidRequest);}
        // This is an SMB2 ERROR, never a successful IOCTL with an empty list.
        // No error data, contexts, FileId, or interface bytes follow the fixed part.
        let mut draft=Self::standalone(member,sid,IOCTL,why as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }

    pub(in crate::smb) fn tree_success(approved:&QualifiedTreeConnect<'_,'_>,sid:NonZeroU64,id:&ProposedTreeId)
        ->Result<Self>{
        let member=approved.original();
        let m=member.message();
        if m.header.routing!=(Routing::Sync{tree:0})||m.header.flags&RELATED!=0||m.raw.get(20..24)!=Some(&[0;4][..]){
            return Err(Error::InvalidRequest);
        }
        let mut draft=Self::from_member(member,sid,TREE_CONNECT,0,TREE_OK,false,false)?;
        draft.raw[36..40].copy_from_slice(&id.get().to_le_bytes());
        draft.raw[64..66].copy_from_slice(&16u16.to_le_bytes());draft.raw[66]=1; // Disk
        draft.raw[68..72].copy_from_slice(&0x30u32.to_le_bytes()); // NO_CACHING; no other properties
        // The actual share ceiling is distinct from the virtual root's
        // permissions and the grant for every individual file and directory.
        let maximum=0x0010_00a1u32|if approved.projection().has_readwrite_child(){0x0001_0006}else{0};
        draft.raw[76..80].copy_from_slice(&maximum.to_le_bytes());
        Ok(draft)
    }

    pub(in crate::smb) fn root_failure(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,why:RootFailure)->Result<Self>{
        let command=member.message().header.command;
        if !matches!(command,CREATE|CLOSE){return Err(Error::InvalidRequest);}
        let mut draft=Self::standalone(member,sid,command,why as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }

    // Even a paid MID with insufficient payload charge is entitled to a
    // signed refusal, but it must not obtain an executable VerifiedMember.
    pub(in crate::smb) fn directory_failure(original:&DirectoryRefusal<'_,'_>)->Result<Self>{
        let (m,sid,why)=original.fields();
        let header=standalone_header(m,sid,14,why as u32)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);raw[64]=9;
        Ok(Self{raw,len:EMPTY_ERROR,sid})
    }

    pub(in crate::smb) fn read_failure(original:&ReadRefusal<'_,'_>)->Result<Self>{
        let (m,sid,why)=original.fields();
        let header=standalone_header(m,sid,super::read_reply::READ,why as u32)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);raw[64]=9;
        Ok(Self{raw,len:EMPTY_ERROR,sid})
    }

    pub(in crate::smb) fn write_refusal(original:&WriteRefusal<'_,'_>)->Result<Self>{
        let (m,sid,why)=original.fields();let command=m.header.command;
        if !matches!(command,WRITE|FLUSH|SET_INFO){return Err(Error::InvalidRequest);}
        let header=standalone_header(m,sid,command,why as u32)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);raw[64]=9;
        Ok(Self{raw,len:EMPTY_ERROR,sid})
    }

    pub(in crate::smb) fn wrote(member:&VerifiedMember<'_,'_>,pending:&PendingWrite)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let receipt=pending.reply();let command=receipt.command();
        let (body,data)=receipt.parts();
        if !matches!((command,body.len()),(WRITE,16)|(FLUSH,4)|(SET_INFO,2))||receipt.status()!=0||!data.is_empty(){
            return Err(Error::InvalidRequest);
        }
        let mut result=Self::standalone(member,pending.sid(),command,0,64+body.len())?;
        result.raw[64..64+body.len()].copy_from_slice(body);Ok(result)
    }

    pub(in crate::smb) fn fs_info_refusal(original:&FsInfoRefusal<'_,'_>)->Result<Self>{
        let (m,sid,why)=original.fields();
        let header=standalone_header(m,sid,QUERY_INFO,why as u32)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);raw[64]=9;
        Ok(Self{raw,len:EMPTY_ERROR,sid})
    }

    pub(in crate::smb) fn rename_refusal(original:&RenameRefusal<'_,'_>)->Result<Self>{
        let (m,sid,why)=original.fields();
        if !m.is_basic_rename_request(){return Err(Error::InvalidRequest);}
        let header=standalone_header(m,sid,SET_INFO,why as u32)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);raw[64]=9;
        Ok(Self{raw,len:EMPTY_ERROR,sid})
    }

    pub(in crate::smb) fn named_renamed(member:&VerifiedMember<'_,'_>,pending:&PendingRename)->Result<Self>{
        if !pending.matches(member)||!member.message().is_basic_rename_request(){return Err(Error::InvalidRequest);}
        let receipt=pending.reply();
        let (body,data)=receipt.parts();
        if receipt.command()!=SET_INFO||receipt.status()!=0||body.len()!=2||!data.is_empty(){return Err(Error::InvalidRequest);}
        let mut draft=Self::standalone(member,pending.sid(),SET_INFO,0,66)?;
        draft.raw[64..66].copy_from_slice(body);Ok(draft)
    }

    pub(in crate::smb) fn root_fs_attributes(member:&VerifiedMember<'_,'_>,pending:&PendingRootAttributes)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let (output,read_only,maximum)=pending.payload();
        let body=verified_fs_attributes(member,output,read_only,maximum)?;
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,body.status,64+body.len)?;
        draft.raw[64..64+body.len].copy_from_slice(&body.bytes[..body.len]);
        Ok(draft)
    }
    pub(in crate::smb) fn root_fs_size(member:&VerifiedMember<'_,'_>,pending:&PendingRootSize)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let (format,output,allocation)=pending.payload();
        let (status,body,len)=verified_fs_size(member,format,output,allocation)?;
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,status,64+len)?;
        draft.raw[64..64+len].copy_from_slice(&body[..len]);Ok(draft)
    }
    pub(in crate::smb) fn file_standard(member:&VerifiedMember<'_,'_>,pending:&PendingFileStandard)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let (output,fields)=pending.payload();
        if info.class!=InfoClass::File(FileClass::Standard)||info.output_bytes!=output||
            matches!(fields,FileStandardFields::File{allocation,eof,..} if allocation>i64::MAX as u64||eof>i64::MAX as u64){
            return Err(Error::InvalidRequest);
        }
        let (status,body,len)=file_standard_body(output,fields);
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,status,64+len)?;
        draft.raw[64..64+len].copy_from_slice(&body[..len]);Ok(draft)
    }
    pub(in crate::smb) fn attribute_tag(member:&VerifiedMember<'_,'_>,pending:&PendingAttributeTag)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let (output,attributes,tag)=pending.values();
        if info.class!=InfoClass::File(FileClass::AttributeTag)||info.output_bytes!=output{return Err(Error::InvalidRequest);}
        let (status,body)=file_attribute_tag_body(output,attributes,tag);
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,status,64+body.len())?;
        draft.raw[64..64+body.len()].copy_from_slice(&body);Ok(draft)
    }
    pub(in crate::smb) fn file_access(member:&VerifiedMember<'_,'_>,pending:&PendingFileAccess)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let (output,grant)=pending.values();
        if info.class!=InfoClass::File(FileClass::Access)||info.output_bytes!=output{return Err(Error::InvalidRequest);}
        let (status,body,len)=file_access_body(output,grant);
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,status,64+len)?;
        draft.raw[64..64+len].copy_from_slice(&body[..len]);Ok(draft)
    }
    pub(in crate::smb) fn internal_info(member:&VerifiedMember<'_,'_>,pending:&PendingInternal)->Result<Self>{
        if !pending.matches(member){return Err(Error::InvalidRequest);}
        let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
        let (output,number)=pending.values();
        if info.class!=InfoClass::File(FileClass::Internal)||info.output_bytes!=output||number==0{
            return Err(Error::InvalidRequest);
        }
        let (status,body)=internal_body(output,number);
        let mut draft=Self::standalone(member,pending.sid(),QUERY_INFO,status,64+body.len())?;
        draft.raw[64..64+body.len()].copy_from_slice(&body);Ok(draft)
    }

    pub(in crate::smb) fn exit_failure(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,why:ExitFailure)->Result<Self>{
        let cmd=member.message().header.command;
        if !matches!(cmd,LOGOFF|TREE_DISCONNECT){return Err(Error::InvalidRequest);}
        let mut draft=Self::standalone(member,sid,cmd,why as u32,EMPTY_ERROR)?;
        if cmd==LOGOFF{draft.raw[36..40].fill(0);}
        draft.raw[64]=9;Ok(draft)
    }

    pub(in crate::smb) fn leaving(member:&VerifiedMember<'_,'_>,plan:&ExitPlan)->Result<Self>{
        if !plan.matches(member){return Err(Error::InvalidRequest);}
        let cmd=member.message().header.command;
        if !matches!(cmd,LOGOFF|TREE_DISCONNECT){return Err(Error::InvalidRequest);}
        let mut draft=Self::standalone(member,plan.sid(),cmd,0,68)?;
        if cmd==LOGOFF{draft.raw[36..40].fill(0);}
        draft.raw[64]=4;Ok(draft)
    }

    pub(in crate::smb) fn opened(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,pending:&PendingOpen)->Result<Self>{
        let (size,maximum)=Self::open_size(member,sid,pending)?;
        let mut draft=Self::standalone(member,sid,CREATE,0,size)?;
        draft.fill_open(pending,maximum)?;
        Ok(draft)
    }
    pub(in crate::smb) fn browse_opened(part:&BrowsePart<'_,'_>,member:&VerifiedMember<'_,'_>,pending:&PendingOpen)->Result<Self>{
        let (_,sid,_,i)=part.fields();
        if i!=0||!part.matches(member){return Err(Error::InvalidRequest);}
        let (size,contexts)=Self::open_size(member,sid,pending)?;
        let mut draft=Self::browse_part(part,CREATE,0,size)?;
        draft.fill_open(pending,contexts)?;
        Ok(draft)
    }
    pub(in crate::smb) fn browse_create_failure(part:&BrowsePart<'_,'_>,why:RootFailure)->Result<Self>{
        let mut draft=Self::browse_part(part,CREATE,why as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    pub(in crate::smb) fn browse_query_failure(part:&BrowsePart<'_,'_>,why:Option<super::query_reply::DirectoryFailure>)->Result<Self>{
        let status=why.map_or(0xc000_0008,|e|e as u32); // too early for a real FileId if absent
        let mut draft=Self::browse_part(part,super::query_reply::DIRECTORY_COMMAND,status,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    fn browse_part(part:&BrowsePart<'_,'_>,command:u16,status:u32,size:usize)->Result<Self>{
        let (m,sid,tree,i)=part.fields();
        let expected=match i{
            0=>CREATE,1=>super::query_reply::DIRECTORY_COMMAND,_=>return Err(Error::InvalidRequest),
        };
        if command!=expected||!matches!(m.header.routing,Routing::Sync{..})||
            (m.header.flags&RELATED!=0)!=(i==1){return Err(Error::InvalidRequest);}
        let len=(size+7)&!7;
        if len>MAX_SMALL_REPLY{return Err(Error::Limit);}
        let header=response_header(m,sid,command,status,if i==0{len as u32}else{0})?;
        let mut raw=[0;MAX_SMALL_REPLY];
        raw[..64].copy_from_slice(&header);
        raw[36..40].copy_from_slice(&tree.to_le_bytes());
        Ok(Self{raw,len,sid})
    }
    fn open_size(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,pending:&PendingOpen)->Result<(usize,OpenContexts)>{
        if !pending.matches(member,sid){return Err(Error::InvalidRequest);}
        let fields=member.message().create_fields().map_err(|_|Error::InvalidRequest)?;
        if !matches!((fields.disposition(),pending.action()),
            (Disposition::Open,1)|(Disposition::OpenIf,1|2)|(Disposition::Create,2)){
            return Err(Error::InvalidRequest);
        }
        let intent=fields.current_intent().map_err(|_|Error::InvalidRequest)?;
        let maximum=match (intent.maximal_access(),pending.maximal_access_answer(intent.name())){
            (None,None)=>None,
            (Some(MaximumAccessQuery::Unconditional),Some(mask))=>Some(mask),
            // No actual response fact means this CREATE cannot succeed,
            // even if the requested context occupied valid input bytes.
            _=>return Err(Error::InvalidRequest),
        };
        let disk_id=match (intent.disk_id(),pending.disk_id_answer(intent.name())){
            (false,None)=>None,
            (true,Some((file,volume))) if file!=0&&volume!=0=>Some((file,volume)),
            _=>return Err(Error::InvalidRequest),
        };
        let contexts=OpenContexts{maximum,disk_id};
        Ok((CREATE_FIXED+contexts.length(),contexts))
    }
    fn fill_open(&mut self,pending:&PendingOpen,contexts:OpenContexts)->Result<()>{
        // Actual opened/created object. No reparse point, oplock, generic context or
        // invented FILETIMEs. A FILE's sizes are from its exact logical view.
        self.raw[64..66].copy_from_slice(&89u16.to_le_bytes());
        self.raw[68..72].copy_from_slice(&pending.action().to_le_bytes());
        let (allocation,eof,attributes)=pending.response_sizes_and_attributes();
        if allocation>i64::MAX as u64||eof>i64::MAX as u64{return Err(Error::Limit);}
        self.raw[104..112].copy_from_slice(&allocation.to_le_bytes());
        self.raw[112..120].copy_from_slice(&eof.to_le_bytes());
        self.raw[120..124].copy_from_slice(&attributes.to_le_bytes());
        let ids=pending.wire();
        self.raw[128..136].copy_from_slice(&ids.persistent.to_le_bytes());
        self.raw[136..144].copy_from_slice(&ids.volatile.to_le_bytes());
        encode_open_contexts(&mut self.raw,contexts);
        Ok(())
    }

    pub(in crate::smb) fn closed(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,closed:&ClosedOpen)->Result<Self>{
        if !closed.matches(member,sid){return Err(Error::InvalidRequest);}
        let mut draft=Self::standalone(member,sid,CLOSE,0,124)?;
        draft.raw[64..124].copy_from_slice(closed.body());
        Ok(draft)
    }

    pub(in crate::smb) fn rename_opened(part:&FileActionPart<'_,'_>,member:&VerifiedMember<'_,'_>,pending:&PendingOpen)->Result<Self>{
        if !part.is_rename()||!(pending.is_retained()||pending.is_read_file()){return Err(Error::InvalidRequest);}
        Self::file_action_opened(part,member,pending)
    }
    pub(in crate::smb) fn disposition_opened(part:&FileActionPart<'_,'_>,member:&VerifiedMember<'_,'_>,pending:&PendingOpen)->Result<Self>{
        if part.is_rename()||!pending.is_read_file(){return Err(Error::InvalidRequest);}
        Self::file_action_opened(part,member,pending)
    }
    fn file_action_opened(part:&FileActionPart<'_,'_>,member:&VerifiedMember<'_,'_>,pending:&PendingOpen)->Result<Self>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let (_,sid,_,_)=part.fields();
        let (size,maximum)=Self::open_size(member,sid,pending)?;
        let mut draft=Self::file_action_part(part,CREATE,0,size)?;
        draft.fill_open(pending,maximum)?;Ok(draft)
    }
    pub(in crate::smb) fn restore_related_done(part:&FileActionPart<'_,'_>,request:&RenameRequest<'_,'_>,evidence:&PreparedRetainedRestore)
        ->Result<Self>{
        if !part.is_rename()||!part.matches(request.original())||
           request.open()!=super::io_fields::OpenReference::PreviousInCompound{return Err(Error::InvalidRequest);}
        let reply=FileReply::completed_restore(evidence);
        Self::file_action_set(part,reply)
    }
    pub(in crate::smb) fn ordinary_rename_related_done(part:&FileActionPart<'_,'_>,request:&RenameRequest<'_,'_>,
        evidence:&PreparedOrdinaryRename)->Result<Self>{
        if !part.is_rename()||!part.matches(request.original())||
           request.open()!=super::io_fields::OpenReference::PreviousInCompound{return Err(Error::InvalidRequest);}
        Self::file_action_set(part,FileReply::completed_rename(evidence))
    }
    pub(in crate::smb) fn disposition_related_done(part:&FileActionPart<'_,'_>,request:&FileSetRequest<'_,'_>,mark:&PreparedDisposition)
        ->Result<Self>{
        if part.is_rename()||!part.matches(request.original())||request.change()!=Change::MarkForDelete||
           request.open()!=super::io_fields::OpenReference::PreviousInCompound{return Err(Error::InvalidRequest);}
        let reply=FileReply::completed_disposition(mark);
        Self::file_action_set(part,reply)
    }
    fn file_action_set(part:&FileActionPart<'_,'_>,reply:FileReply<'_>)->Result<Self>{
        let (body,data)=reply.parts();
        if reply.status()!=0||reply.command()!=SET_INFO||body.len()!=2||!data.is_empty(){return Err(Error::InvalidRequest);}
        let mut draft=Self::file_action_part(part,SET_INFO,0,66)?;
        draft.raw[64..66].copy_from_slice(body);Ok(draft)
    }
    pub(in crate::smb) fn file_action_closed(part:&FileActionPart<'_,'_>,member:&VerifiedMember<'_,'_>,done:&CloseCompletion)
        ->Result<Self>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let reply=FileReply::close(done);
        let (body,data)=reply.parts();
        if reply.status()!=0||reply.command()!=CLOSE||body.len()!=60||!data.is_empty(){return Err(Error::InvalidRequest);}
        let mut draft=Self::file_action_part(part,CLOSE,0,124)?;
        draft.raw[64..124].copy_from_slice(body);Ok(draft)
    }
    pub(in crate::smb) fn file_action_create_refusal(part:&FileActionPart<'_,'_>,error:RootFailure)->Result<Self>{
        Self::file_action_refusal(part,CREATE,error as u32)
    }
    pub(in crate::smb) fn file_action_without_open(part:&FileActionPart<'_,'_>)->Result<Self>{
        let (m,_,_,_)=part.fields();
        if !matches!(m.header.command,SET_INFO|CLOSE){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,m.header.command,0xc000_0008)
    }
    pub(in crate::smb) fn rename_related_failure(part:&FileActionPart<'_,'_>,error:RestoreFailure)->Result<Self>{
        if !part.is_rename(){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,SET_INFO,error as u32)
    }
    pub(in crate::smb) fn rename_close_blocked(part:&FileActionPart<'_,'_>,error:RestoreFailure)->Result<Self>{
        if !part.is_rename(){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,CLOSE,error as u32)
    }
    pub(in crate::smb) fn disposition_related_failure(part:&FileActionPart<'_,'_>,error:WriteFailure)->Result<Self>{
        if part.is_rename(){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,SET_INFO,error as u32)
    }
    pub(in crate::smb) fn disposition_close_blocked(part:&FileActionPart<'_,'_>,error:WriteFailure)->Result<Self>{
        if part.is_rename(){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,CLOSE,error as u32)
    }
    pub(in crate::smb) fn disposition_close_failure(part:&FileActionPart<'_,'_>,error:RootFailure)->Result<Self>{
        if part.is_rename(){return Err(Error::InvalidRequest);}
        Self::file_action_refusal(part,CLOSE,error as u32)
    }
    fn file_action_refusal(part:&FileActionPart<'_,'_>,cmd:u16,error:u32)->Result<Self>{
        let mut draft=Self::file_action_part(part,cmd,error,EMPTY_ERROR)?;draft.raw[64]=9;Ok(draft)
    }
    fn file_action_part(part:&FileActionPart<'_,'_>,cmd:u16,status:u32,size:usize)->Result<Self>{
        let (m,sid,tree,i)=part.fields();
        let expected=match i{0=>CREATE,1=>SET_INFO,2=>CLOSE,_=>return Err(Error::InvalidRequest)};
        if cmd!=expected||(cmd==SET_INFO&&!part.selects_set())||
           !matches!(m.header.routing,Routing::Sync{..})||
           (m.header.flags&RELATED!=0)!=(i>0){return Err(Error::InvalidRequest);}
        let (len,next)=probe_reply_layout(size,i);
        if len>MAX_SMALL_REPLY{return Err(Error::Limit);}
        let header=response_header(m,sid,cmd,status,next)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);
        raw[36..40].copy_from_slice(&tree.to_le_bytes());
        Ok(Self{raw,len,sid})
    }

    pub(in crate::smb) fn probe_opened(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,pending:&PendingOpen)->Result<Self>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let (_,sid,_,_)=part.fields();
        let (size,maximum)=Self::open_size(member,sid,pending)?;
        let mut draft=Self::probe_part(part,CREATE,0,size)?;
        draft.fill_open(pending,maximum)?;Ok(draft)
    }
    pub(in crate::smb) fn probe_fs_attributes(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,prepared:&PreparedRootFsAttributes)->Result<Self>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let (read_only,maximum)=prepared.policy();
        let body=verified_fs_attributes(member,prepared.output(),read_only,maximum)?;
        // This chain allows the normal QUERY_INFO payload or its partial
        // name warning, never a failed query turned into a CLOSE receipt.
        if body.status>>30==3{return Err(Error::InvalidRequest);}
        let mut draft=Self::probe_part(part,QUERY_INFO,body.status,64+body.len)?;
        draft.raw[64..64+body.len].copy_from_slice(&body.bytes[..body.len]);Ok(draft)
    }
    pub(in crate::smb) fn probe_fs_size(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,prepared:&PreparedRootFsSize)
        ->Result<(Self,Option<FsInfoFailure>)>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let (format,output,units)=prepared.values();
        let (status,body,len)=verified_fs_size(member,format,output,units)?;
        let failure=match status{
            0=>None,
            0xc000_0004=>Some(FsInfoFailure::InformationLengthMismatch),
            0xc000_00a3=>Some(FsInfoFailure::DeviceNotReady),
            _=>return Err(Error::InvalidRequest),
        };
        let mut draft=Self::probe_part(part,QUERY_INFO,status,64+len)?;
        draft.raw[64..64+len].copy_from_slice(&body[..len]);Ok((draft,failure))
    }
    pub(in crate::smb) fn probe_size_refusal(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,error:FsInfoFailure)
        ->Result<Self>{
        if !part.matches(member)||!member.message().query_info(member.max_transact())
            .is_ok_and(|q|q.class.capacity_format().is_some())||
            matches!(error,FsInfoFailure::InformationLengthMismatch|FsInfoFailure::DeviceNotReady){
            return Err(Error::InvalidRequest);
        }
        // Fixed-short/no-media require their original prepared proof above.
        let mut draft=Self::probe_part(part,QUERY_INFO,error as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    pub(in crate::smb) fn probe_all_refusal(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,error:FsInfoFailure)->Result<Self>{
        if !part.matches(member)||!member.message().query_info(member.max_transact())
            .is_ok_and(|q|q.class==InfoClass::File(FileClass::All))||
           !matches!(error,FsInfoFailure::AccessDenied|FsInfoFailure::NotSupported){
            return Err(Error::InvalidRequest);
        }
        let mut draft=Self::probe_part(part,QUERY_INFO,error as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    pub(in crate::smb) fn probe_close_blocked(part:&ProbePart<'_,'_>,error:FsInfoFailure)->Result<Self>{
        let mut draft=Self::probe_part(part,CLOSE,error as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    pub(in crate::smb) fn probe_closed(part:&ProbePart<'_,'_>,member:&VerifiedMember<'_,'_>,done:CloseCompletion)->Result<Self>{
        if !part.matches(member){return Err(Error::InvalidRequest);}
        let reply=FileReply::close(&done);let (body,data)=reply.parts();
        if reply.command()!=CLOSE||reply.status()!=0||body.len()!=60||!data.is_empty(){return Err(Error::InvalidRequest);}
        let mut draft=Self::probe_part(part,CLOSE,0,124)?;
        draft.raw[64..124].copy_from_slice(body);Ok(draft)
    }
    pub(in crate::smb) fn probe_create_refusal(part:&ProbePart<'_,'_>,error:RootFailure)->Result<Self>{
        let mut draft=Self::probe_part(part,CREATE,error as u32,EMPTY_ERROR)?;
        draft.raw[64]=9;Ok(draft)
    }
    pub(in crate::smb) fn probe_without_open(part:&ProbePart<'_,'_>)->Result<Self>{
        let (m,_,_,_)=part.fields();
        let cmd=m.header.command;
        if !matches!(cmd,QUERY_INFO|CLOSE){return Err(Error::InvalidRequest);}
        let mut draft=Self::probe_part(part,cmd,0xc000_0008,EMPTY_ERROR)?; // STATUS_INVALID_HANDLE
        draft.raw[64]=9;Ok(draft)
    }
    fn probe_part(part:&ProbePart<'_,'_>,cmd:u16,status:u32,size:usize)->Result<Self>{
        let (m,sid,tree,i)=part.fields();
        let (expected,more)=part.reply_shape().ok_or(Error::InvalidRequest)?;
        if cmd!=expected||!matches!(m.header.routing,Routing::Sync{..})||
           (m.header.flags&RELATED!=0)!=(i>0){return Err(Error::InvalidRequest);}
        let len=(size+7)&!7;
        let next=if more{len as u32}else{0};
        if len>MAX_SMALL_REPLY{return Err(Error::Limit);}
        let header=response_header(m,sid,cmd,status,next)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);
        // Reconstruct from the effective original chain. Later literal
        // all-ones (or a different valid TreeId) never decide this value.
        raw[36..40].copy_from_slice(&tree.to_le_bytes());
        Ok(Self{raw,len,sid})
    }

    fn standalone(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,command:u16,status:u32,len:usize)->Result<Self>{
        if !member.response_session_matches(sid){return Err(Error::InvalidRequest);}
        let header=standalone_header(member.message(),sid,command,status)?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);Ok(Self{raw,len,sid})
    }

    fn from_member(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,command:u16,
                   status:u32,len:usize,compound:bool,more:bool)->Result<Self> {
        if !member.response_session_matches(sid)||(more&&!compound){return Err(Error::InvalidRequest);}
        let header=response_header(member.message(),sid,command,status,if more{len as u32}else{0})?;
        let mut raw=[0;MAX_SMALL_REPLY];raw[..64].copy_from_slice(&header);
        Ok(Self{raw,len,sid})
    }

    pub(in crate::smb) fn credit(mut self,grant:u16)->PayableSmall {
        self.raw[14..16].copy_from_slice(&grant.to_le_bytes());PayableSmall(self)
    }
}

// MS-SMB2 §§2.2.14.1, .2.5 and .2.9. No caller can supply an arbitrary
// context or emit these fields independently of the proven original OPEN.
fn encode_open_contexts(raw:&mut [u8;MAX_SMALL_REPLY],known:OpenContexts){
    let n=known.length();
    if n==0{return;}
    raw[144..148].copy_from_slice(&(CREATE_FIXED as u32).to_le_bytes());
    raw[148..152].copy_from_slice(&(n as u32).to_le_bytes());
    let mut at=CREATE_FIXED;
    if let Some(mask)=known.maximum{
        let context=&mut raw[at..at+MAXIMAL_CONTEXT];
        let next=if known.disk_id.is_some(){MAXIMAL_CONTEXT as u32}else{0};
        context_prefix(context,b"MxAc",8,next);
        // QueryStatus=SUCCESS indicated by the zero first data DWORD.
        context[28..32].copy_from_slice(&mask.to_le_bytes());
        at+=MAXIMAL_CONTEXT;
    }
    if let Some((file,volume))=known.disk_id{
        let context=&mut raw[at..at+QUERY_DISK_CONTEXT];
        context_prefix(context,b"QFid",32,0);
        context[24..32].copy_from_slice(&file.to_le_bytes());
        context[32..40].copy_from_slice(&volume.to_le_bytes());
        // Reserved 16 bytes (40..56) stay zero.
    }
}
fn context_prefix(context:&mut [u8],tag:&[u8;4],data_length:u32,next:u32){
    context.fill(0);
    context[..4].copy_from_slice(&next.to_le_bytes());
    context[4..6].copy_from_slice(&16u16.to_le_bytes());
    context[6..8].copy_from_slice(&4u16.to_le_bytes());
    context[10..12].copy_from_slice(&24u16.to_le_bytes());
    context[12..16].copy_from_slice(&data_length.to_le_bytes());
    context[16..20].copy_from_slice(tag);
}

// MS-SMB2 §3.3.4.1.3 pads the last element too; the final Next is
// still zero and its full zero tail belongs to that member's CMAC.
fn probe_reply_layout(size:usize,ordinal:usize)->(usize,u32){
    let len=(size+7)&!7;
    (len,if ordinal<2{len as u32}else{0})
}
struct FsAttributesBody{status:u32,len:usize,bytes:[u8;26]}
fn verified_fs_attributes(member:&VerifiedMember<'_,'_>,output:u32,read_only:bool,maximum:u32)->Result<FsAttributesBody>{
    let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
    if output!=info.output_bytes||info.class!=InfoClass::Filesystem(FsClass::Attribute)||
       !(1..=255).contains(&maximum){return Err(Error::InvalidRequest);}
    Ok(root_fs_attributes_body(output,read_only,maximum))
}
pub(super) fn information_too_small(raw:&mut [u8]){
    // 3.1.1 default error context: exactly one eight-byte header
    // (both ErrorDataLength and ErrorId zero), no payload.
    raw[..16].fill(0);raw[0]=9;raw[2]=1;raw[4]=8;
}
// MS-FSCC §2.4.1 and MS-FSA §2.1.5.12.1: one unsigned LE ACCESS_MASK.
fn file_access_body(output:u32,grant:u32)->(u32,[u8;16],usize){
    let mut bytes=[0;16];bytes[0]=9;
    if output<4{
        information_too_small(&mut bytes);
        return (0xc000_0004,bytes,16);
    }
    bytes[2..4].copy_from_slice(&72u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&4u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&grant.to_le_bytes());
    (0,bytes,12)
}
// MS-FSCC §2.4.27: one little-endian file number.
fn internal_body(output:u32,number:u64)->(u32,[u8;16]){
    fixed_file_info8_body(output,number.to_le_bytes())
}
// MS-FSCC §2.4.6: two little-endian 32-bit fields, no variable tail.
fn file_attribute_tag_body(output:u32,attributes:u32,tag:u32)->(u32,[u8;16]){
    let mut data=[0;8];
    data[..4].copy_from_slice(&attributes.to_le_bytes());data[4..].copy_from_slice(&tag.to_le_bytes());
    fixed_file_info8_body(output,data)
}
// MS-SMB2 §3.3.5.20.1 shared fixed-length framing/error, not a signing API.
fn fixed_file_info8_body(output:u32,data:[u8;8])->(u32,[u8;16]){
    let mut bytes=[0;16];bytes[0]=9;
    if output<8{
        information_too_small(&mut bytes);
        return (0xc000_0004,bytes);
    }
    bytes[2..4].copy_from_slice(&72u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&8u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&data);
    (0,bytes)
}
// MS-SMB2 §3.3.5.20.2 and MS-FSCC §§2.5.4/2.5.8. Neither has a variable tail.
fn verified_fs_size(member:&VerifiedMember<'_,'_>,format:CapacityFormat,output:u32,units:Option<CapacityUnits>)
    ->Result<(u32,[u8;40],usize)>{
    let info=member.message().query_info(member.max_transact()).map_err(|_|Error::InvalidRequest)?;
    if output!=info.output_bytes||info.class.capacity_format()!=Some(format){return Err(Error::InvalidRequest);}
    root_fs_size_body(format,output,units)
}
fn root_fs_size_body(format:CapacityFormat,output:u32,units:Option<CapacityUnits>)->Result<(u32,[u8;40],usize)>{
    let mut body=[0;40];body[0]=9;
    let fixed=format.bytes();
    if output<fixed{
        if units.is_some(){return Err(Error::InvalidRequest);}
        information_too_small(&mut body);
        return Ok((0xc000_0004,body,16));
    }
    let Some(CapacityUnits{total,caller_available,actual_free})=units else{
        return Ok((0xc000_00a3,body,8)); // DEVICE_NOT_READY, not a zero volume
    };
    if total==0||caller_available>actual_free||actual_free>total||total>i64::MAX as u64{return Err(Error::InvalidRequest);}
    body[2..4].copy_from_slice(&72u16.to_le_bytes());
    body[4..8].copy_from_slice(&fixed.to_le_bytes());
    body[8..16].copy_from_slice(&total.to_le_bytes());
    body[16..24].copy_from_slice(&caller_available.to_le_bytes());
    let geometry=match format{
        CapacityFormat::Size=>24,
        CapacityFormat::FullSize=>{body[24..32].copy_from_slice(&actual_free.to_le_bytes());32},
    };
    // Same ZXS 4 KiB reporting unit in both forms, not physical-disk geometry.
    body[geometry..geometry+4].copy_from_slice(&8u32.to_le_bytes());
    body[geometry+4..geometry+8].copy_from_slice(&512u32.to_le_bytes());
    Ok((0,body,8+fixed as usize))
}
#[derive(Clone,Copy)]
pub(in crate::smb) enum FileStandardFields{File{allocation:u64,eof:u64,delete_pending:bool},Directory}

// MS-SMB2 §3.3.5.20.1, MS-FSCC §2.4.47. No trailing variable data.
fn file_standard_body(output:u32,fields:FileStandardFields)->(u32,[u8;32],usize){
    let mut bytes=[0;32];bytes[0]=9;
    if output<24{
        information_too_small(&mut bytes);
        return (0xc000_0004,bytes,16);
    }
    bytes[2..4].copy_from_slice(&72u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&24u32.to_le_bytes());
    match fields{
        FileStandardFields::File{allocation,eof,delete_pending}=>{
            bytes[8..16].copy_from_slice(&allocation.to_le_bytes());
            bytes[16..24].copy_from_slice(&eof.to_le_bytes());
            // The real delete mark removes the one live logical link from
            // this count, not from storage before the normal final CLOSE.
            bytes[24]=u8::from(!delete_pending);
            bytes[28]=u8::from(delete_pending);
        },
        FileStandardFields::Directory=>{
            bytes[24]=1; // selected ordinary path, no hard links or pending deletion
            bytes[29]=1; // directory without a logical byte stream
        },
    }
    (0,bytes,32)
}
// MS-SMB2 §3.3.5.20.2, MS-FSCC §2.5.1, MS-FSA §2.1.5.13.5.
// Only the narrow checked dispatcher above can turn the bytes into a reply.
fn root_fs_attributes_body(output:u32,read_only:bool,maximum:u32)->FsAttributesBody{
    const NAME:[u8;6]=[b'Z',0,b'X',0,b'S',0]; // no terminator or physical FS claim
    const FIXED:usize=12;
    let mut bytes=[0;26];bytes[0]=9;
    if output<FIXED as u32{
        information_too_small(&mut bytes);
        return FsAttributesBody{status:0xc000_0004,len:16,bytes};
    }
    let n=(output as usize-FIXED).min(NAME.len());
    bytes[2..4].copy_from_slice(&72u16.to_le_bytes());
    bytes[4..8].copy_from_slice(&((FIXED+n) as u32).to_le_bytes());
    // The ZXS namespace is case-sensitive, case-preserving and Unicode.
    // File/volume extensions would be independent capabilities.
    let flags=0x0000_0007u32 | if read_only{0x0008_0000}else{0};
    bytes[8..12].copy_from_slice(&flags.to_le_bytes());
    bytes[12..16].copy_from_slice(&maximum.to_le_bytes());
    bytes[16..20].copy_from_slice(&(NAME.len() as u32).to_le_bytes());
    bytes[20..20+n].copy_from_slice(&NAME[..n]);
    FsAttributesBody{status:if n<NAME.len(){0x8000_0005}else{0},len:20+n,bytes}
}
#[derive(Clone,Copy)]
#[repr(u32)]
pub(in crate::smb) enum ExitFailure{InvalidParameter=0xc000_000d,NetworkNameDeleted=0xc000_00c9}
// The shared header builder is deliberately not a signing or sending API.
pub(super) fn standalone_header(m:&Message<'_>,sid:NonZeroU64,command:u16,status:u32)->Result<[u8;64]>{
    if !matches!(m.header.routing,Routing::Sync{..})||m.header.flags&RELATED!=0||
        m.raw.get(20..24)!=Some(&[0;4][..]){return Err(Error::InvalidRequest);}
    response_header(m,sid,command,status,0)
}
pub(super) fn response_header(m:&Message<'_>,sid:NonZeroU64,command:u16,status:u32,next:u32)->Result<[u8;64]>{
    if m.header.command!=command||m.header.flags&SIGNED==0||
        (m.header.flags&RELATED==0&&m.header.session!=sid.get()){return Err(Error::InvalidRequest);}
    // Always start with the original request's immutable header bytes.
    let mut raw:[u8;64]=m.raw.get(..64).ok_or(Error::InvalidRequest)?.try_into().map_err(|_|Error::InvalidRequest)?;
    raw[8..12].copy_from_slice(&status.to_le_bytes());
    raw[14..16].fill(0); // grant only from the originating ledger
    raw[16..20].copy_from_slice(&(m.header.flags|1|SIGNED).to_le_bytes());
    raw[20..24].copy_from_slice(&next.to_le_bytes());
    raw[40..48].copy_from_slice(&sid.get().to_le_bytes());raw[48..64].fill(0);
    Ok(raw)
}
impl PayableSmall {
    pub(in crate::smb) fn sid(&self)->NonZeroU64 {self.0.sid}
    pub(in crate::smb) fn for_mac(&self)->&[u8] {&self.0.raw[..self.0.len]}
    pub(in crate::smb) fn signed(mut self,tag:[u8;16])->SignedSmall {
        self.0.raw[48..64].copy_from_slice(&tag);SignedSmall(self.0)
    }
}
impl SignedSmall {pub(in crate::smb) fn bytes(&self)->&[u8] {&self.0.raw[..self.0.len]}}

#[cfg(test)]
#[path="../../test/smb/root_mxac_reply.rs"]
mod tests;

#[cfg(test)]
#[path="../../test/smb/root_fs_info_reply.rs"]
mod fs_info_tests;

#[cfg(test)]
#[path="../../test/smb/file_standard_reply.rs"]
mod file_info_tests;
