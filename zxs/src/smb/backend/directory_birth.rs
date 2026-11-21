//! A directory birth gets the same actual original OPEN admission and a
//! real DirectoryId share vote before creating the exact kind-13 object.
use super::{BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,Pending,ReserveError,Share,
    directory_open::{DIR_OPTIONS,DirectoryOpen,DirectoryOpenError as OpenError,readonly_grant},
    path::{NameError,TreeName}};
use crate::{DirectoryId,PoolId,smb::{RustSessionWitness,
    connection::sessions::ordinary::BirthAdmission,
    wire::create_fields::{CurrentName,Disposition}},
    system::reader::smb::{RustNamedDirectory,directory::RustOrdinaryDirectory,runtime::RustRoot}};
use std::{io,sync::Arc};

const FILE_DIRECTORY_FILE:u32=1;

struct Opening<'a>{tree:&'a RustRoot,scope:&'a RustSessionWitness,grant:Grant}
impl Opening<'_>{
    fn planned(&self,id:DirectoryId)->Facts{
        Facts{object:Object::Directory(self.tree.pool(),id),grant:self.grant,parent_write:ParentWrite::Granted}
    }
}
impl OriginalRequest for Opening<'_>{
    type Held=RustOrdinaryDirectory;
    type Error=OpenError;
    fn recheck(&self,held:&Self::Held)->Result<Facts,OpenError>{
        if !held.belongs_to(self.tree)||!held.parent_allows_file_write(){return Err(OpenError::WrongTree);}
        held.require_current(self.tree,self.scope).map_err(OpenError::Original)?;
        let id=held.nested().ok_or(OpenError::WrongTree)?;
        Ok(self.planned(id))
    }
}

/// The actual shared coordinator's provisional vote for this precise
/// DirectoryId. Cannot be VOLUME-ROOT, FILE, or the target of a rename.
#[must_use]
pub(crate) struct DirectoryBirthVote<'q,'r>{pending:Pending<'q,Opening<'r>>}
impl DirectoryBirthVote<'_,'_>{
    pub(crate) fn is_provisional_for(&self,pool:PoolId,id:DirectoryId)->bool{
        !self.pending.slot.bound&&!self.pending.slot.released&&self.pending.planned.object==Object::Directory(pool,id)
    }
    fn bind(self,original:RustOrdinaryDirectory)->io::Result<Held<RustOrdinaryDirectory>>{
        self.pending.bind(original).map_err(|err|match err{
            BindError::Original(OpenError::Original(e))=>io::Error::new(io::ErrorKind::ConnectionAborted,e),
            _=>io::ErrorKind::ConnectionAborted.into(),
        })
    }
}
#[must_use]
pub(in crate::smb) enum PreparedDirectoryBirth{Opened(DirectoryOpen),Created(DirectoryOpen)}

impl OpenCoordinator{
    pub(in crate::smb) fn prepare_directory_birth(self:&Arc<Self>,admission:BirthAdmission<'_,'_,'_>)
        ->Result<PreparedDirectoryBirth,OpenError>{
        let (tree,member)=admission.into_original();
        let fields=member.message().create_fields().map_err(OpenError::Fields)?;
        let intent=fields.current_intent().map_err(OpenError::Special)?;
        let CurrentName::Relative(raw)=intent.name() else{return Err(OpenError::UnsupportedOptions);};
        let open_if=match fields.disposition(){
            Disposition::Create=>false,Disposition::OpenIf=>true,
            _=>return Err(OpenError::UnsupportedOptions),
        };
        if intent.disk_id()||intent.maximal_access().is_some(){return Err(OpenError::ContextRequired);}
        let options=fields.effective_options();
        if options&FILE_DIRECTORY_FILE==0||options&!DIR_OPTIONS!=0{return Err(OpenError::UnsupportedOptions);}
        let grant=readonly_grant(fields.desired_access())?;
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(OpenError::AccessDenied)?;
        let scope=member.rust_session_witness().map_err(OpenError::Session)?;
        let request=Opening{tree,scope:&scope,grant};
        let target=TreeName::create_relative(raw).map_err(OpenError::Name)?;
        let mut names=target.components();
        let label=names.next().ok_or(OpenError::Name(NameError::InvalidName))?;
        let leaf=names.next_back().ok_or(OpenError::Name(NameError::InvalidName))?;
        let mut parents=Vec::new();
        for name in names{
            if parents.len()>=crate::fs::MAX_PATH_DEPTH-1{return Err(OpenError::Name(NameError::TooLong));}
            parents.try_reserve(1).map_err(|_|OpenError::Resources)?;
            parents.push(name);
        }
        let candidate=tree.choose_directory_birth_or_existing(&scope,label,&parents,leaf,open_if).map_err(OpenError::Original)?;
        match candidate{
            RustNamedDirectory::Existing(existing)=>{
                let id=existing.nested().ok_or(OpenError::WrongTree)?;
                let vote=self.reserve(&request,request.planned(id),share).map_err(reserve_failure)?;
                let actual=existing.acquire().map_err(OpenError::Original)?;
                let held=vote.bind(actual).map_err(|e|match e{
                    BindError::Original(e)=>e,BindError::Changed=>OpenError::WrongTree,
                })?;
                Ok(PreparedDirectoryBirth::Opened(DirectoryOpen::from_bound(held)))
            },
            RustNamedDirectory::Vacant(vacant)=>{
                let mut refused=None;
                let result=vacant.complete(|id|match self.reserve(&request,request.planned(id),share){
                    Ok(pending)=>Ok(DirectoryBirthVote{pending}),
                    Err(e)=>{refused=Some(e);Err(io::ErrorKind::PermissionDenied.into())},
                });
                if let Some(e)=refused{return Err(reserve_failure(e));}
                let (actual,vote)=result.map_err(OpenError::Original)?;
                let held=vote.bind(actual).map_err(OpenError::Original)?;
                Ok(PreparedDirectoryBirth::Created(DirectoryOpen::from_bound(held)))
            },
        }
    }
}
fn reserve_failure(e:ReserveError)->OpenError{
    match e{
        ReserveError::SharingViolation=>OpenError::ShareViolation,
        ReserveError::DeletePending=>OpenError::DeletePending,
        ReserveError::Resources=>OpenError::Resources,
        ReserveError::Unavailable=>OpenError::Original(io::Error::other("unavailable original directory OPEN coordinator")),
    }
}
