//! Nonsecret display selections for the sole Web administrator. No directory
//! names, raw paths, password material, SMB token, disk or FILE FD escape.
use super::{WebAdmin,share_mode::{self,Mode}};
use crate::{OperationId,PoolId,Role,VolumeId,identity::ShareId};
use std::io;

pub(crate) struct Overview{
    pub(crate) pool:PoolId,pub(crate) volumes:Vec<(Role,VolumeId)>,
    pub(crate) web:String,pub(crate) smb:String,pub(crate) shares:Vec<Share>,pub(crate) next:Option<ShareId>,
}
pub(crate) struct Share{pub(crate) share:ShareId,pub(crate) previous:OperationId,pub(crate) label:String,pub(crate) volume:VolumeId,pub(crate) mode:Mode}
impl WebAdmin<'_>{
    pub(crate) fn overview(&self,after:Option<ShareId>)->io::Result<Overview>{
        self.source.at(|db,c|{
            let identity=self.current(db,&c)?; // still this original credential, same short cut as the displayed facts
            let (rows,next)=share_mode::page(db,&c,after)?;
            let shares=rows.into_iter().map(|(state,label)|Share{share:state.share,previous:state.op,label,volume:state.volume,mode:state.mode}).collect();
            let volumes=c.rows().iter().map(|r|(r.role,r.volume)).collect();
            Ok(Overview{pool:c.birth.pool,volumes,web:identity.login,smb:identity.smb_login,shares,next})
        })
    }
}
