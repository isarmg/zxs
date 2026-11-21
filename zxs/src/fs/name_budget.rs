//! Geometry of one exact directory, never authority over its contents. A
//! stored component limit is usable only from the verified original SYSTEM.
use super::{HeldDir,PathBudget,invalid,path_budget};
use std::{ffi::CStr,fs::File,io,os::{fd::AsRawFd,raw::c_int}};

unsafe extern "C"{fn zxs_component_limit(fd:c_int)->c_int;}

/// A strict nonzero byte encodes min(the observed native limit, 255). There
/// is no mapping from "could not determine" to the most permissive value.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct ComponentLimit(u8);
impl ComponentLimit{
    pub(crate) fn from_original(value:u8)->io::Result<Self>{
        if value==0{return Err(invalid("missing original native component limit"));}Ok(Self(value))
    }
    pub(crate) fn encoded(self)->u8{self.0}
    pub(crate) fn held(dir:&HeldDir)->io::Result<Self>{
        dir.require_named()?;
        let limit=Self::on_fd(&dir.file)?;
        dir.require_named()?;
        Ok(limit)
    }
    /// Numerical geometry only. A supervisor already holding/proving its
    /// original named mount can sample that same FD; no path or grant here.
    pub(crate) fn on_fd(dir:&File)->io::Result<Self>{
        // SAFETY: this fd stays owned and open across the read-only fpathconf.
        let value=unsafe{zxs_component_limit(dir.as_raw_fd())};
        if value<0{return Err(io::Error::last_os_error());}
        let bounded=u8::try_from(value).map_err(|_|invalid("invalid native component-limit response"))?;
        Self::from_original(bounded)
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct NameBudget{parent:PathBudget,limit:ComponentLimit}
impl NameBudget{
    /// A numerical budget, including when projected from an authenticated
    /// but currently offline root. It cannot open an object or certify a role.
    pub(crate) fn from_original(parent:PathBudget,limit:ComponentLimit)->Self{Self{parent,limit}}
    pub(crate) fn held(dir:&HeldDir)->io::Result<Self>{
        Ok(Self::from_original(path_budget(&dir.path)?,ComponentLimit::held(dir)?))
    }
    pub(crate) fn child(self,leaf:&CStr)->io::Result<PathBudget>{
        if leaf.to_bytes().len()>usize::from(self.limit.0){return Err(invalid("component exceeds the selected parent's native limit"));}
        self.parent.child(leaf)
    }
}
