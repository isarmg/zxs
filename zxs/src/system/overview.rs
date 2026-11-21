//! Business share choices for the sole Web administrator. No host paths,
//! password material, SMB token or physical-volume diagnostic facts escape.
use super::{WebAdmin,share_mode::{self,Mode}};
use crate::{OperationId,VolumeId,identity::ShareId};
use std::io;

pub(crate) struct Overview{
    pub(crate) web:String,pub(crate) smb:String,pub(crate) shares:Vec<Share>,pub(crate) next:Option<ShareId>,
}
pub(crate) struct Share{pub(crate) share:ShareId,pub(crate) previous:OperationId,pub(crate) label:String,pub(crate) volume:VolumeId,pub(crate) mode:Mode}
impl WebAdmin<'_>{
    pub(crate) fn overview(&self,after:Option<ShareId>)->io::Result<Overview>{
        self.source.at(|db,c|{
            let identity=self.current(db,&c)?; // still this original credential, same short cut as the displayed facts
            let (rows,next)=share_mode::page(db,&c,after)?;
            let shares=rows.into_iter().map(|(state,label)|Share{share:state.share,previous:state.op,label,volume:state.volume,mode:state.mode}).collect();
            Ok(Overview{web:identity.login,smb:identity.smb_login,shares,next})
        })
    }
}
