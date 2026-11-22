//! The same live v17 Core credential and immutable PLAN receipts, read by
//! the isolated low worker *before* the root parent selects F/R. This cut
//! returns no VAULT, password/hash, fd, session, tree or write grant.
#![allow(dead_code)] // Only the authenticated preflight entry may connect this.
use super::{ActiveRustMeta,Connection,Catalog,InitialReader,Mode,RustPlan,denied};
use crate::system::reader::credential::Pair;
use crate::{Entity,PoolId,Role,VolumeId,fs,system::{ClaimRow,lease::SystemDir,reader::zxs_sql_path_limit}};
use std::{fs::File,io,path::{Path,PathBuf}};

/// The parent provides exact public facts from the independently approved
/// site and a clone of its held S projected read-only into the low view.
/// Do not forward a host directory fd into this namespace.
pub(crate) struct PreflightRead<'a>{
    pub(crate) system:&'a Path,
    pub(crate) original_system:Entity,
    pub(crate) install:&'a [u8],
    pub(crate) approved_install:[u8;32],
    pub(crate) original_init_actor:[u8;16],
    pub(crate) storage_uid:u32,
}

/// Immutable public comparison facts, not a storage or launch grant.
pub(crate) struct PreflightPlan{
    pub(crate) origin:[[u8;16];5],pub(crate) ordinary:Vec<VolumeId>,
    hostname:String,
    labels:Vec<(String,bool)>,recycle:String,
}
fn approved_plan(raw:&[u8],expected:[u8;32])->io::Result<RustPlan>{
    let p=RustPlan::parse(raw)?;
    if expected==[0;32]||p.plan.sha()!=expected{return Err(denied());}Ok(p)
}
impl PreflightPlan{
    pub(crate) fn decode(raw:&[u8],expected:[u8;32])->io::Result<Self>{
        let p=approved_plan(raw,expected)?;
        let (ordinary,labels)=p.plan.folders().iter().map(|f|(f.volume(),(f.name().to_owned(),f.mode==Mode::ReadWrite)))
            .unzip();
        Ok(Self{origin:p.plan.scope,ordinary,labels,recycle:p.plan.recycle.clone(),
            hostname:p.hostname().to_owned()})
    }
    pub(crate) fn hostname(&self)->&str{&self.hostname}
    /// PLAN order with the enrolled initial modes only, not current authority.
    pub(crate) fn planned_folders(&self)->impl ExactSizeIterator<Item=(VolumeId,&str,bool)>+'_{
        self.ordinary.iter().copied().zip(&self.labels).map(|(v,(name,rw))|(v,name.as_str(),*rw))
    }
    pub(crate) fn recycle_label(&self)->&str{&self.recycle}
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct PreflightEpoch{
    pub(crate) account:[u8;16],pub(crate) op:[u8;16],pub(crate) record:[u8;16],
    pub(crate) sid:[u8;28],pub(crate) generation:i64,pub(crate) login:String,
}
impl PreflightEpoch{
    fn from_current(active:ActiveRustMeta)->Self{
        Self{account:active.canonical.0,op:active.epoch.op.0,record:active.record,sid:active.sid,
            generation:active.epoch.generation,login:active.login}
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct PreflightClaim{
    pub(crate) ordinal:u8,pub(crate) volume:VolumeId,pub(crate) path:PathBuf,
    pub(crate) component_limit:u8,pub(crate) marker:[u8;101],
}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct PreflightSelection{
    pub(crate) origin:[[u8;16];5],pub(crate) install_sha:[u8;32],
    pub(crate) epoch:PreflightEpoch,pub(crate) ordinary:Vec<PreflightClaim>,pub(crate) recycle:PreflightClaim,
}
impl RustPlan{
    pub(super) fn preflight_at(&self,db:&Connection,c:&Catalog,actor:[u8;16],digest:[u8;32])
        ->io::Result<PreflightSelection>{
        if actor==[0;16]||actor!=c.birth.actor||digest!=self.plan.sha(){return Err(denied());}
        let active=self.active_at(db,c)?;
        let claim=|row:&ClaimRow|->io::Result<PreflightClaim>{
            if row.volume.0==self.plan.scope[0]{return Err(denied());}
            let marker=c.expected_marker(usize::from(row.ordinal)).ok_or_else(denied)?;
            Ok(PreflightClaim{ordinal:row.ordinal,volume:row.volume,path:row.path.clone(),component_limit:row.limit.encoded(),marker})
        };
        let recycle=claim(c.rows().get(1).filter(|r|r.role==Role::Recycle&&r.ordinal==1).ok_or_else(denied)?)?;
        let ordinary=self.plan.folders().iter().map(|f|{
            let row=c.rows().iter().find(|r|r.role==Role::File&&r.ordinal>=2&&r.volume==f.volume()).ok_or_else(denied)?;
            claim(row)
        }).collect::<io::Result<Vec<_>>>()?;
        Ok(PreflightSelection{origin:self.plan.scope,install_sha:active.install_sha,
            epoch:PreflightEpoch::from_current(active),ordinary,recycle})
    }
}

fn require_projection(file:&File,observed:Entity,expected:Entity,read_only:bool)->io::Result<()>{
    if observed!=expected||expected.inode==0{return Err(denied());}
    if fs::MountFlags::on_fd(file)?.read_only()!=read_only{return Err(denied());}
    Ok(())
}

fn open_projected(w:&PreflightRead<'_>,read_only:bool,live:&impl Fn()->io::Result<()>)
    ->io::Result<(InitialReader,RustPlan)>{
    if matches!(w.storage_uid,0|65534|u32::MAX)||fs::local_uid()!=w.storage_uid||
        w.approved_install==[0;32]||w.original_init_actor==[0;16]{return Err(denied());}
    let plan=approved_plan(w.install,w.approved_install)?;
    live()?;
    // SAFETY: ask the pinned SQLite Unix VFS for its fixed pathname bound.
    let limit=usize::try_from(unsafe{zxs_sql_path_limit()}).map_err(|_|denied())?;
    let dir=SystemDir::hold_qualified(w.system,limit,|root|{
        require_projection(&root.file,root.entity,w.original_system,read_only)?;
        live()
    })?;
    require_projection(&dir.root.file,dir.root.entity,w.original_system,read_only)?;
    live()?;
    let source=InitialReader::open_held(dir,PoolId(plan.plan.scope[0]))?;
    Ok((source,plan))
}

fn checked_scope(source:&InitialReader,plan:&RustPlan,w:&PreflightRead<'_>,read_only:bool,web:bool,
    live:&impl Fn()->io::Result<()>)->io::Result<PreflightSelection>{
    let reached=||{
        require_projection(&source.dir.root.file,source.dir.root.entity,w.original_system,read_only)?;
        live()
    };
    let selected=source.transaction_before_commit(&source.conn,c"BEGIN",|db,c|{
        let selected=plan.preflight_at(db,&c,w.original_init_actor,w.approved_install)?;
        if web{
            // Web alone can qualify/load its salted verifier in this very
            // snapshot. No verifier bytes are in the returned startup facts.
            let Some((account,epoch,_verifier))=Pair::qualified_web(db,&c)? else{return Err(denied());};
            if (account.canonical.0,epoch.op,epoch.generation,epoch.record,account.login.as_str())!=
               (selected.epoch.account,selected.epoch.op,selected.epoch.generation,selected.epoch.record,selected.epoch.login.as_str())
            {return Err(denied());}
        }
        Ok(selected)
    },&reached)?;
    source.dir.require()?;source.marker.require(&source.dir.root)?;
    reached()?;
    Ok(selected)
}

/// Caller has authenticated the actual local parent and checked its own
/// irreversible credentials/view. The low probe holds an exact read-only S
/// and never opens the vault, F/R or a write DB.
pub(crate) fn preflight_read(w:PreflightRead<'_>,live:impl Fn()->io::Result<()>)->io::Result<PreflightSelection>{
    let (source,plan)=open_projected(&w,true,&live)?;
    checked_scope(&source,&plan,&w,true,false,&live)
}

/// The normal ZXS application opens Web on the *same* original site and
/// SYSTEM. Return the already validated reader, never a path to reopen.
/// The private Web child must have separately matched its original process
/// and socket. A read-only original stays read-only, not silently upgraded.
pub(crate) fn web_reader(w:PreflightRead<'_>,read_only:bool,live:impl Fn()->io::Result<()>)->io::Result<(InitialReader,PreflightEpoch)>{
    let (source,plan)=open_projected(&w,read_only,&live)?;
    let selected=checked_scope(&source,&plan,&w,read_only,true,&live)?;
    Ok((source,selected.epoch))
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/smb_rust_preflight/tests.rs"]
mod tests;
