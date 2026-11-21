//! Reference-volume policy shared by the Native and Rust SMB roles.
//! These are physical observations, never a session or a file grant.
use super::InitialReader;
use super::super::{Catalog,share_mode::{Mode,Setting},volume::{Capacity,OrdinaryRoot}};
use std::io;

pub(super) struct Reference<'r>{
    pub(super) root:OrdinaryRoot<'r>,pub(super) grant:Setting,
    pub(super) bytes:Capacity,pub(super) writable:bool,
}
/// Sample only authorized ordinary candidates, never an aggregate volume.
/// Plan order is stable by original VolumeId inside each class. A SQL RW
/// entry on a read-only mount is only a fallback, never RW.
pub(super) fn pick_reference<'r>(src:&'r InitialReader,catalog:&Catalog,mut choices:Vec<Setting>,
    can_write:bool,live:impl Fn()->io::Result<()>)->io::Result<Option<Reference<'r>>>{
    choices.retain(|s|s.mode!=Mode::Disabled);
    if can_write{choices.sort_by_key(|s|s.mode!=Mode::ReadWrite);}
    let mut chosen=None;
    for grant in choices{
        live()?;
        if chosen.is_some() && grant.mode!=Mode::ReadWrite{break;}
        let Ok(root)=OrdinaryRoot::for_body(src,catalog,grant.volume)else{continue;};
        let Ok(bytes)=root.sampled_capacity()else{continue;};
        if bytes.total<4096{continue;}
        let writable=can_write&&grant.mode==Mode::ReadWrite&&!bytes.read_only;
        if writable||chosen.is_none(){chosen=Some(Reference{root,grant,bytes,writable});}
        if writable||!can_write{break;}
    }
    Ok(chosen)
}
