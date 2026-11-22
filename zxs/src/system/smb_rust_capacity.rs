//! The reference-volume rule of the logical ZXS root.
//! Holds one actual ordinary F, never R or an aggregate, no FILE write right.
use super::{Catalog,Connection,Plan,RustRoot,RustRootStamp,Setting,denied};
use super::super::reference_capacity::pick_reference;
use crate::{Role,VolumeId,smb::RustSessionWitness,system::{ClaimRow,reader::volume::{Capacity,OrdinaryMedia}}};
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

struct Measured{media:OrdinaryMedia,claim:ClaimRow,grant:Setting,sample:Capacity,writable:bool}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct CapacityUnits{
    pub(crate) total:u64,pub(crate) caller_available:u64,pub(crate) actual_free:u64,
}
impl CapacityUnits{
    fn from_sample(sample:&Capacity,writable:bool)->Self{
        Self{total:sample.total/4096,
            caller_available:if writable&&!sample.read_only{sample.available/4096}else{0},
            actual_free:sample.free/4096}
    }
}

/// Qualified point-in-time observation, not media availability forever or
/// permission to operate on that F. Keeps the original F through the send.
#[must_use]
pub(crate) struct RustRootCapacity{
    root:RustRootStamp,
    choices:Vec<Setting>,
    measured:Option<Measured>,
    require_same_mount_mode:bool,
    _permit:Permit,
}
impl RustRootCapacity{
    /// Wire units are always the product's 4 KiB reporting unit. This is
    /// neither the actual sector size nor a reservation of free bytes.
    pub(crate) fn allocation_units(&self)->Option<CapacityUnits>{
        self.measured.as_ref().map(|m|CapacityUnits::from_sample(&m.sample,m.writable))
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
    pub(crate) fn sample_capacity(&self,query:&RustSessionWitness,require_same_mount_mode:bool)->io::Result<RustRootCapacity>{
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
                Some(Measured{media:p.root.into_media(),claim,grant:p.grant,sample:p.bytes,writable:p.writable})
            },
        };
        let result=RustRootCapacity{root:self.stamp(),choices,measured,require_same_mount_mode,_permit:permit};
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
            if original.require_same_mount_mode{
                m.media.capacity_origin().require_mount_mode(m.sample.read_only)?;
            }
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

#[cfg(test)]
#[path = "../../tests/unit/attached/system/smb_rust_capacity/tests.rs"]
mod tests;
