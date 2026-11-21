//! Same reference-volume rule as the existing Native logical ZXS root.
//! Holds one actual ordinary F, never R or an aggregate, no FILE write right.
use super::{Catalog,Connection,Plan,RustRoot,RustRootStamp,Setting,denied};
use super::super::reference_capacity::pick_reference;
use crate::{Role,VolumeId,smb::RustSessionWitness,system::{ClaimRow,reader::volume::OrdinaryMedia}};
use std::{io,sync::atomic::{AtomicUsize,Ordering}};

// Each connection is synchronous: at most one capacity turn on its owner.
// Constrain potentially stalled filesystem samples/checks to two lanes in
// this service process. This is not an OS disk-operation timeout.
static ACTIVE:AtomicUsize=AtomicUsize::new(0);
struct Permit;
impl Permit{
    fn try_take()->io::Result<Self>{
        ACTIVE.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<2).then_some(n+1))
            .map_err(|_|io::ErrorKind::WouldBlock)?;
        Ok(Self)
    }
}
impl Drop for Permit{fn drop(&mut self){ACTIVE.fetch_sub(1,Ordering::AcqRel);}}

struct Measured{media:OrdinaryMedia,claim:ClaimRow,grant:Setting,total:u64,available:u64}

/// Qualified point-in-time observation, not media availability forever or
/// permission to operate on that F. Keeps the original F through the send.
#[must_use]
pub(crate) struct RustRootCapacity{
    root:RustRootStamp,
    choices:Vec<Setting>,
    measured:Option<Measured>,
    _permit:Permit,
}
impl RustRootCapacity{
    /// Wire units are always the product's 4 KiB reporting unit. This is
    /// neither the actual sector size nor a reservation of free bytes.
    pub(crate) fn allocation_units(&self)->Option<(u64,u64)>{
        self.measured.as_ref().map(|m|(m.total/4096,m.available/4096))
    }
}

fn row(c:&Catalog,volume:VolumeId)->io::Result<&ClaimRow>{
    c.rows().iter().find(|r|r.role==Role::File&&r.volume==volume).ok_or_else(denied)
}
impl Plan{
    fn capacity_choices(&self,db:&Connection,c:&Catalog)->io::Result<Vec<Setting>>{
        self.folders().iter().map(|f|self.current(db,c,f.name())?.ok_or_else(denied)).collect()
    }
}
impl RustRoot{
    pub(crate) fn sample_capacity(&self,query:&RustSessionWitness)->io::Result<RustRootCapacity>{
        if !query.is_query_info(){return Err(denied());}
        self.require_member(query)?;
        let permit=Permit::try_take()?;
        let site=&self.install;
        let plan=&site.installed.plan;
        let (catalog,choices)=site.source.at(|db,c|{
            if site.installed.active_at(db,&c)?!=site.original{return Err(denied());}
            let choices=plan.capacity_choices(db,&c)?;
            Ok((c,choices))
        })?;
        // The only SMB account has the share's current write policy, not
        // write access to the virtual root. Current mode AND physical RO
        // decide whether the sampled F contributes available allocation.
        let selected=pick_reference(&site.source,&catalog,choices.clone(),true,||self.require_member(query))?;
        let measured=match selected{
            None=>None,
            Some(p)=>{
                if !p.root.for_tree(&site.source,p.grant.volume)||p.bytes.available>p.bytes.free||
                   p.bytes.free>p.bytes.total{return Err(denied());}
                let claim=row(&catalog,p.grant.volume)?.clone();
                Some(Measured{media:p.root.into_media(),claim,grant:p.grant,total:p.bytes.total,
                    available:if p.writable{p.bytes.available}else{0}})
            },
        };
        let result=RustRootCapacity{root:self.stamp(),choices,measured,_permit:permit};
        self.require_capacity(query,&result)?;Ok(result)
    }

    pub(crate) fn require_capacity(&self,query:&RustSessionWitness,original:&RustRootCapacity)->io::Result<()>{
        if !query.is_query_info()||!self.matches_stamp(&original.root){return Err(denied());}
        self.require_member(query)?;
        let site=&self.install;
        // Original F/mark tests can block. Never put them inside the
        // SQLite cut or replace them with a second sample or fresh F.
        if let Some(m)=&original.measured{
            m.media.view(&site.source).require_streaming()?;
        }
        site.source.at(|db,c|{
            if site.installed.active_at(db,&c)?!=site.original||
               site.installed.plan.capacity_choices(db,&c)?!=original.choices{return Err(denied());}
            if let Some(m)=&original.measured{
                if row(&c,m.grant.volume)?!=&m.claim{return Err(denied());}
                m.media.capacity_origin().require_catalog(&c,m.grant.volume)?;
            }
            Ok(())
        })?;
        self.require_member(query)
    }
}
