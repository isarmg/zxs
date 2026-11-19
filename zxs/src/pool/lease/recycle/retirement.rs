//! Only the two addresses and exact original FILE selected by the confirmed
//! operation. No raw recycle root or arbitrary deletion/rename callback.
use super::{Access,CustodyEnd};
use crate::{fs::{HeldDir,invalid},ordinary::{LifecycleChecked,LifecycleOriginals,LifecyclePrevious,LifecycleRetiring}};
use std::io;
impl CustodyEnd<'_,'_>{
    fn old_pair(&self,key:&LifecyclePrevious)->io::Result<(Option<HeldDir>,Option<HeldDir>)>{
        self.require_current()?;let(b,q)=key.names(self.pool,self.grant,self)?;
        let pair=(self.directory().exact_directory(&b)?,self.directory().exact_directory(&q)?);self.require_current()?;Ok(pair)
    }
    pub(crate) fn capture_retirement(&self,key:&LifecyclePrevious)->io::Result<LifecycleOriginals>{
        let(b,q)=self.old_pair(key)?;let raw=key.pin(self.pool,self.grant,self,b,q)?;
        let(b,q)=self.old_pair(key)?;raw.matches(b,q)?;self.require_current()?;Ok(raw)
    }
    pub(crate) fn check_retiring(&self,v:&LifecycleRetiring)->io::Result<()>{
        let(b,q)=self.old_pair(v.previous())?;v.matches(b,q)?;self.require_current()
    }
    pub(crate) fn sync_absent_business(&self,v:&LifecycleRetiring)->io::Result<()>{
        if !matches!(self.pool.access,Access::Write){return Err(invalid("not original retirement writer"));}
        self.check_retiring(v)?;let(b,_)=v.previous().names(self.pool,self.grant,self)?;
        self.directory().confirm_absence(&b)?;self.directory().file.sync_all()?;self.check_retiring(v)
    }
    pub(crate) fn isolate_original(&self,v:&LifecycleRetiring,dest:&CustodyEnd<'_,'_>,checked:&LifecycleChecked,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let(e,birth)=v.isolation(self.pool,self.grant,self,dest,checked)?;
        let(b,q)=v.previous().names(self.pool,self.grant,self)?;
        self.directory().handoff_directory(&b,self.directory(),&q,e,&birth,after)
    }
    pub(crate) fn release_empty(&self,v:&LifecycleRetiring,dir:&HeldDir,dest:&CustodyEnd<'_,'_>,checked:&LifecycleChecked,after:impl FnOnce()->io::Result<()>)->io::Result<()>{
        let birth=v.empty_witness(self.pool,self.grant,self,dest,checked,dir)?;
        let(_,q)=v.previous().names(self.pool,self.grant,self)?;self.directory().retire_empty_original(&q,dir,&birth,after)
    }
    pub(crate) fn confirm_gone(&self,v:&LifecycleRetiring)->io::Result<()>{
        v.can_observe_gone(self.pool,self.grant,self)?;let(b,q)=v.previous().names(self.pool,self.grant,self)?;
        self.directory().confirm_absence(&b)?;self.directory().confirm_absence(&q)?;self.check_retiring(v)
    }
}
