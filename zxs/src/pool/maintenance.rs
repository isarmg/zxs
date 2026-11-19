//! Pure first-registration input. A successful decode is neither a locked
//! register nor a business/maintenance capability. No paths are opened here.
use super::*;

const GENESIS:&[u8]=b"ZXS/FILE-POOL/MAINT-1\0";
const LABEL:&[u8]=b"ZXS/VOLUME/MAINT-1\0";

/// Zero frontier, no journal tail, exact labels in the original role order.
/// The observation belongs to the configured storage owner, not a web user.
pub(in crate::pool) fn read_empty(bytes:&[u8],storage_uid:u32,frontier:&[u8],labels:&[&[u8]])->io::Result<PoolInfo>{
    if bytes.len()>MAX_RECORD{return Err(invalid_record());}
    let(r,tail)=Record::decode_prefix(bytes,GENESIS)?;
    if !tail.is_empty()||r.actor_uid!=storage_uid||frontier!=fs::frontier(0,0,r.digest()).as_slice()||
        labels.len()!=r.info.volumes.len(){return Err(invalid_record());}
    let paths:BTreeSet<&Path>=r.info.volumes.iter().map(|v|v.volume.path.as_path()).collect();
    let mut births=BTreeSet::from([&r.origin_birth]);
    for(i,v)in r.info.volumes.iter().enumerate(){
        let path=&v.volume.path;
        if v.volume.entity==r.origin||!births.insert(&r.roots[i])||
            path.ancestors().skip(1).any(|ancestor|paths.contains(ancestor))||
            labels[i]!=r.label_in_domain(i,LABEL).as_slice(){return Err(invalid_record());}
    }
    Ok(r.info)
}

#[cfg(test)]
#[path="../../tests/unit/maintenance_genesis.rs"]
mod tests;
