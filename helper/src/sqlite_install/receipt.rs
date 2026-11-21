//! Read the exact original INIT on the once-selected SYSTEM alone. No data
//! volume is opened, inferred from SQL, synchronized or granted a new role.
use super::{approval,bad,catalog::{self,Canonical,Catalog},engine::{self,Access},markers::SystemMark,
    originals::{LockedSystem,SelectedSystem},request::{Id,Request},reservation::Existing,DATABASE,MARKER};
use std::{io,path::PathBuf};

#[derive(Debug,Eq,PartialEq)]
pub(super) struct Receipt{pub(super) pool:Id,pub(super) committed:bool}

pub(super) fn foreground(op:Id,reference:Id,paths:Vec<PathBuf>,subject:super::pair::ExplicitSubject)->io::Result<Receipt>{
    super::pair::compiled_v17()?;
    approval::no_inherited_descriptors()?;
    let host=approval::host_admin()?;
    let (actor,uid,gid)=subject.tuple();
    approval::same_terminal(&io::stdin(),&io::stderr())?;
    let original=Request::new(op,actor,reference,uid,paths)?;
    let path=crate::pinned::Pinned::already_mounted(&original.paths()[0],crate::pinned::Kind::StorageRoot)?;
    let system=SelectedSystem::from_origin(path,host,uid,gid)?;
    approval::same_terminal(&io::stdin(),&io::stderr())?;
    // Actual SQLite and marker content are opened only after permanently
    // lowering to the same configured storage identity as the birth action.
    approval::drop_to_storage(uid,gid)?;
    let locked=system.lock()?;
    let observed=read(&locked,&original)?;
    approval::same_terminal(&io::stdin(),&io::stderr())?;
    Ok(observed)
}

pub(super) fn read(system:&LockedSystem,original:&Request)->io::Result<Receipt>{
    let template=Canonical::new()?;
    let db=Existing::initial_receipt(system,original)?;
    let source=system.source();
    let slots=||source.controls(&[DATABASE.as_bytes(),MARKER]);
    let initial=slots()?;
    if !initial[0]{return Err(bad("the original INIT database is not present"));}
    let marker=if initial[1]{Some(SystemMark::before_sql(source)?)}else{None};
    let verify=|claim:&Catalog|->io::Result<()>{
        system.require()?;db.sidecars_absent()?;
        if slots()?!=initial{return Err(bad("the original SYSTEM controls changed during this receipt inquiry"));}
        match &marker{
            Some(m)=>m.verify(source,claim.markers.first().ok_or_else(||bad("missing original SYSTEM claim"))?),
            None if claim.committed=>Err(bad("committed INIT lacks its required original SYSTEM marker")),
            None=>Ok(()),
        }
    };
    let failure=|_|bad("this original INIT receipt could not be confirmed");
    let mut c=engine::existing(&db,Access::Read)?;
    let tx=c.transaction().map_err(failure)?;
    let claim=catalog::initial_receipt(&tx,&template,source,original)?;
    verify(&claim)?;engine::original_name(&tx)?;
    tx.commit().map_err(failure)?;
    engine::original_name(&c)?;verify(&claim)?;
    c.close().map_err(|_|bad("could not close the original read-only INIT observation"))?;
    verify(&claim)?;
    Ok(Receipt{pool:claim.pool,committed:claim.committed})
}
