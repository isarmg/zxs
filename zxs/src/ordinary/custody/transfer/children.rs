//! Exact ordered formal children at a custody cut. There is no "second
//! without first" representation; zero and one keep their original encoding.
use crate::{fs::invalid,ordinary::wire::In};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum ChildRoles<T>{None,One(T),Two(T,T)}
impl<T> ChildRoles<T>{
    pub(in crate::ordinary) fn from_first(v:Option<T>)->Self{match v{None=>Self::None,Some(a)=>Self::One(a)}}
    pub(in crate::ordinary) fn first(&self)->Option<&T>{match self{Self::None=>None,Self::One(a)|Self::Two(a,_)=>Some(a)}}
    pub(in crate::ordinary) fn second(&self)->Option<&T>{match self{Self::Two(_,b)=>Some(b),_=>None}}
    pub(in crate::ordinary) fn len(&self)->usize{match self{Self::None=>0,Self::One(_)=>1,Self::Two(..)=>2}}
    pub(in crate::ordinary) fn iter(&self)->impl Iterator<Item=&T>{self.first().into_iter().chain(self.second())}
    pub(in crate::ordinary) fn ordinal(&self,i:usize)->Option<&T>{match i{0=>self.first(),1=>self.second(),_=>None}}
    pub(in crate::ordinary) fn as_refs(&self)->ChildRoles<&T>{match self{
        Self::None=>ChildRoles::None,Self::One(a)=>ChildRoles::One(a),Self::Two(a,b)=>ChildRoles::Two(a,b)}}
    pub(in crate::ordinary) fn map<U>(self,mut f:impl FnMut(T)->U)->ChildRoles<U>{match self{
        Self::None=>ChildRoles::None,Self::One(a)=>ChildRoles::One(f(a)),Self::Two(a,b)=>ChildRoles::Two(f(a),f(b))}}
    pub(super) fn put(&self,b:&mut Vec<u8>,mut put:impl FnMut(&T,&mut Vec<u8>)){
        b.push(self.len() as u8);for item in self.iter(){put(item,b);}
    }
    pub(super) fn get(r:&mut In<'_>,mut read:impl FnMut(&mut In<'_>)->io::Result<T>)->io::Result<Self>{
        Ok(match r.byte()?{0=>Self::None,1=>Self::One(read(r)?),2=>Self::Two(read(r)?,read(r)?),
            _=>return Err(invalid("unknown count of retained formal children"))})
    }
}
