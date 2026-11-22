//! One opt-in controller log for the exact selected PCIe namespace.
use super::{Dev, Field, Reason, aliases::NvmeNamespace, kernel::{NvmeOwner, Sys}};
use helper_linux_evidence::nvme_health::{self, Failure, LOG_BYTES};
use rustix::io::Errno;
use std::{ffi::OsString, fmt, os::unix::ffi::OsStrExt};

#[derive(Debug)]
pub(crate) struct HealthQuery(Dev);
impl HealthQuery {
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        if !args.next().is_some_and(|x|x=="--device"){return Err("expected --device MAJOR:MINOR");}
        let dev=args.next().and_then(|x|Dev::parse(x.as_bytes())).ok_or("expected one canonical MAJOR:MINOR")?;
        if args.next().is_some(){return Err("health accepts no other device, flags or controls");}
        Ok(Self(dev))
    }

    fn observe(&self)->Field<Health>{
        let sys=Sys::open().map_err(Reason::io)?;
        let node=sys.pin(self.0).map_err(Reason::io)?;
        let identity=node.identity(&sys).map_err(Reason::io)?;
        let owner=NvmeOwner::select(&sys,&node,&identity)?;
        let device=NvmeNamespace::open(&identity)?;
        owner.require(&sys)?;
        let seen=nvme_health::read(device.original(),self.0.rdev(),owner.generation());
        device.require()?;
        owner.require(&sys)?;
        if node.identity(&sys).map_err(Reason::io)?!=identity{return Err(Reason::Changed);}
        Health::decode(&seen.map_err(reason)?)
    }

    pub(crate) fn run(self)->(u8,String){
        match self.observe(){
            Ok(health)=>(0,format!("device={} scope=one-PCIe-NVMe-controller (not per-volume)\n{health}",self.0)),
            Err(why)=>(2,format!("device={} nvme-health=unknown({})",self.0,why.label())),
        }
    }
}

fn reason(failure:Failure)->Reason{
    match failure{
        Failure::Changed=>Reason::Changed,
        Failure::Unsupported=>Reason::Unsupported,
        Failure::Io(err) if err.raw_os_error().is_some_and(|n|[Errno::NOTTY,Errno::OPNOTSUPP,Errno::NOSYS]
            .iter().any(|e|e.raw_os_error()==n))=>Reason::Unsupported,
        Failure::Io(err)=>Reason::io(err),
    }
}

#[derive(Debug,Eq,PartialEq)]
struct Health {
    warnings:u8,
    temperature_kelvin:Option<u16>,
    available_spare:u8,
    spare_threshold:u8,
    endurance_used:u8,
    media_errors:u128,
}
impl Health {
    fn decode(page:&[u8;LOG_BYTES])->Field<Self>{
        let [warnings,a,b,available_spare,spare_threshold,endurance_used,..]=*page;
        if available_spare>100||spare_threshold>100{return Err(Reason::Invalid);}
        let absolute=u16::from_le_bytes([a,b]);
        let errors=page[160..176].try_into().expect("one NVMe log counter");
        Ok(Self{warnings,temperature_kelvin:(absolute!=0).then_some(absolute),available_spare,
            spare_threshold,endurance_used,media_errors:u128::from_le_bytes(errors)})
    }
}
impl fmt::Display for Health {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let temp=self.temperature_kelvin.map_or_else(||"not-reported".into(),|v|v.to_string());
        write!(f,"critical-warning=0x{:02x} temperature-K={} spare-percent={} spare-warning-threshold-percent={} \
            endurance-used-percent={} media-errors={}",self.warnings,temp,self.available_spare,self.spare_threshold,
            self.endurance_used,self.media_errors)
    }
}

#[cfg(test)]
#[path="../../tests/block_tools/health.rs"]
mod tests;
