//! The first-registration parser is pure. Native cold read admission is isolated
//! in native; decoded bytes alone are never a business or maintenance grant.
use super::*;

const GENESIS:&[u8]=b"ZXS/FILE-POOL/MAINT-1\0";
const LABEL:&[u8]=b"ZXS/VOLUME/MAINT-1\0";

/// Zero frontier, no journal tail, exact labels in the original role order.
/// The observation belongs to the configured storage owner, not a web user.
pub(in crate::pool) fn read_empty(bytes:&[u8],storage_uid:u32,frontier:&[u8],labels:&[&[u8]])->io::Result<PoolInfo>{
    let r=decode_empty(bytes,storage_uid,frontier)?;
    if labels.len()!=r.info.volumes.len()||labels.iter().enumerate().any(|(i,l)|*l!=r.label_in_domain(i,LABEL).as_slice()){
        return Err(invalid_record());}
    Ok(r.info)
}

fn decode_empty(bytes:&[u8],storage_uid:u32,frontier:&[u8])->io::Result<Record>{
    if bytes.len()>MAX_RECORD{return Err(invalid_record());}
    let(r,tail)=decode_genesis(bytes,storage_uid)?;
    require_empty_tail(&r,tail,frontier)?;Ok(r)
}
fn require_empty_tail(r:&Record,tail:&[u8],frontier:&[u8])->io::Result<()>{
    if !tail.is_empty()||frontier!=fs::frontier(0,0,r.digest()).as_slice(){return Err(invalid_record());}
    Ok(())
}
fn decode_genesis(bytes:&[u8],storage_uid:u32)->io::Result<(Record,&[u8])>{
    let(r,tail)=Record::decode_prefix(bytes,GENESIS)?;
    if r.bytes.len()>MAX_RECORD||r.actor_uid!=storage_uid{return Err(invalid_record());}
    let paths:BTreeSet<&Path>=r.info.volumes.iter().map(|v|v.volume.path.as_path()).collect();
    let mut births=BTreeSet::from([&r.origin_birth]);
    for(i,v)in r.info.volumes.iter().enumerate(){
        if v.volume.entity==r.origin||!births.insert(&r.roots[i])||
            v.volume.path.ancestors().skip(1).any(|p|paths.contains(p)){return Err(invalid_record());}
    }
    Ok((r,tail))
}

#[path="maintenance_open.rs"]
pub(in crate::pool) mod native;

#[cfg(test)]
#[path="../../tests/unit/maintenance_genesis.rs"]
mod tests;
