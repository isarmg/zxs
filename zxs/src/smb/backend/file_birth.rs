//! The original signed writable FILE create and its pre-existing share votes.
//! No CREATE unless the protocol owner already holds a real undisclosed seat.
use super::{BindError,Facts,Grant,Held,Object,OpenCoordinator,OriginalRequest,ParentWrite,Pending,ReserveError,Share,
    disposition::DELETE_ON_CLOSE,file_open::{FILE_OPTIONS,FileAnswers,FileOpenError,MAXIMUM_ALLOWED,with_path},
    writable_file_open::{WritableFileOpen,explicit_writer,write_grant}};
use crate::{FileId,PoolId,smb::{RustSessionWitness,
    connection::sessions::ordinary::BirthAdmission,
    wire::create_fields::{CurrentName,Disposition}},
    system::reader::smb::{RustNamedWrite,RustStagedWriter,runtime::RustRoot}};
use std::{io,sync::Arc};

struct Opening<'a>{tree:&'a RustRoot,scope:&'a RustSessionWitness,grant:Grant}
impl Opening<'_>{
    fn facts(&self,file:FileId)->Facts{
        Facts{object:Object::File(self.tree.pool(),file),grant:self.grant,parent_write:ParentWrite::Granted}
    }
}
impl OriginalRequest for Opening<'_>{
    type Held=RustStagedWriter;
    type Error=FileOpenError;
    fn recheck(&self,held:&Self::Held)->Result<Facts,Self::Error>{
        if !held.belongs_to(self.tree){return Err(FileOpenError::WrongTree);}
        held.require_current(self.tree,self.scope).map_err(FileOpenError::Original)?;
        Ok(self.facts(held.file()))
    }
}

/// Only an actual coordinator can make this pending FILE vote. Its
/// identity and grant are not a freestanding CREATE permission.
#[must_use]
pub(crate) struct FileBirthVote<'q,'r>{pending:Pending<'q,Opening<'r>>}
impl FileBirthVote<'_,'_>{
    pub(crate) fn is_provisional_for(&self,pool:PoolId,file:FileId)->bool{
        !self.pending.slot.bound&&!self.pending.slot.released&&self.pending.planned.object==Object::File(pool,file)
    }
    fn bind(self,stage:RustStagedWriter)->io::Result<Held<RustStagedWriter>>{
        self.pending.bind(stage).map_err(|err|match err{
            BindError::Original(FileOpenError::Original(e))=>io::Error::new(io::ErrorKind::ConnectionAborted,e),
            _=>io::ErrorKind::ConnectionAborted.into(),
        })
    }
}

#[must_use]
pub(in crate::smb) enum PreparedBirth{Opened(WritableFileOpen),Created(WritableFileOpen)}

impl OpenCoordinator{
    pub(in crate::smb) fn prepare_file_birth(self:&Arc<Self>,admission:BirthAdmission<'_,'_,'_>)
        ->Result<PreparedBirth,FileOpenError>{
        let (tree,member)=admission.into_original();
        let fields=member.message().create_fields().map_err(FileOpenError::Fields)?;
        let intent=fields.current_intent().map_err(FileOpenError::Special)?;
        let CurrentName::Relative(raw)=intent.name() else{return Err(FileOpenError::NotFile);};
        let open_if=match fields.disposition(){
            Disposition::Create=>false,Disposition::OpenIf=>true,
            _=>return Err(FileOpenError::UnsupportedOptions),
        };
        let answers=FileAnswers::for_intent(intent)?;
        let options=fields.effective_options();
        // No directory birth, overwrite, or create-then-delete lane.
        if options&!(FILE_OPTIONS&!DELETE_ON_CLOSE)!=0{return Err(FileOpenError::UnsupportedOptions);}
        let desired=fields.desired_access();
        if !explicit_writer(desired)&&desired&MAXIMUM_ALLOWED==0{return Err(FileOpenError::AccessDenied);}
        let grant=write_grant(desired)?;
        let share=Share::from_bits(u32::from(fields.share_access())).ok_or(FileOpenError::AccessDenied)?;
        let scope=member.rust_session_witness().map_err(FileOpenError::Session)?;
        let write_through=options&2!=0;
        let chosen=with_path(raw,|label,parents,name|{
            tree.choose_named_birth_or_writer(&scope,label,parents,name,open_if).map_err(FileOpenError::Original)
        })?;
        match chosen{
            RustNamedWrite::Existing(original)=>{
                self.bind_writable(tree,&scope,original,grant,share,write_through,false,answers).map(PreparedBirth::Opened)
            },
            RustNamedWrite::Vacant(actual)=>{
                let opening=Opening{tree,scope:&scope,grant};
                let mut refused=None;
                let result=actual.complete(|file|{
                    match self.reserve(&opening,opening.facts(file),share){
                        Ok(pending)=>Ok(FileBirthVote{pending}),
                        Err(e)=>{
                            refused=Some(e);
                            Err(io::ErrorKind::PermissionDenied.into())
                        },
                    }
                });
                if let Some(e)=refused{return Err(reserve_failure(e));}
                let (stage,vote)=result.map_err(FileOpenError::Original)?;
                let original=vote.bind(stage).map_err(FileOpenError::Original)?;
                // The birth is already durable. An unavailable original
                // number cannot become a definite "not created" result.
                WritableFileOpen::from_bound(original,write_through,answers,tree,&scope)
                    .map(PreparedBirth::Created)
                    .map_err(|e|FileOpenError::Original(io::Error::new(io::ErrorKind::ConnectionAborted,e)))
            },
        }
    }
}
fn reserve_failure(e:ReserveError)->FileOpenError{
    match e{
        ReserveError::SharingViolation=>FileOpenError::ShareViolation,
        ReserveError::DeletePending=>FileOpenError::DeletePending,
        ReserveError::Resources=>FileOpenError::Resources,
        ReserveError::Unavailable=>FileOpenError::Original(io::Error::other("unavailable original FILE share coordinator")),
    }
}
