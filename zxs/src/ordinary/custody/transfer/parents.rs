//! Exact ordinary parent choices in a custody request and the original
//! directory evidence written beside them. A recycle endpoint is not an
//! ordinary parent. None of these values can be used for physical I/O.
use super::{Call,In,NativeObject,Sites};
use crate::{DirectoryId,FileId,OperationId,PoolId,VolumeId,fs::invalid,
    ordinary::{directory::types::Parent,file_parent::BirthParent as Anchor}};
use std::io;

/// The empty (root-only) form is the explicit meaning of existing method
/// tags 0..3. A scoped tag 4..7 must name at least one real child.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Parents{
    Roots,
    Move{source:Anchor,target:Anchor},
    Delete{source:Anchor},
    Restore{deleted_from:Parent,target:Anchor},
}

/// Only selectors, never native handles, enter user request identity. A
/// damaged/replaced directory is not admitted by matching its original ID.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum Choice{
    Roots,
    Move{source:Parent,target:Parent},
    Delete{source:Parent},
    Restore{deleted_from:Parent,target:Parent},
}
/// A registered recycle endpoint has no ordinary name index at all.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) enum End{Ordinary(Parent),Recycle(VolumeId)}
impl End{
    pub(in crate::ordinary) fn ordinary(self)->Option<Parent>{match self{Self::Ordinary(p)=>Some(p),Self::Recycle(_)=>None}}
    pub(in crate::ordinary) fn volume(self)->VolumeId{match self{Self::Ordinary(p)=>p.volume(),Self::Recycle(v)=>v}}
}
fn child(p:Parent)->Option<DirectoryId>{match p{Parent::Child(_,d)=>Some(d),Parent::Root(_)=>None}}
impl Choice{
    fn check(self,call:Call,from:VolumeId,to:VolumeId)->io::Result<()>{
        let ok=match(self,call){
            (Self::Roots,_)=>true,
            (Self::Move{source,target},Call::Move)=>source.volume()==from&&target.volume()==to&&
                (child(source).is_some()||child(target).is_some())&&
                !child(source).is_some_and(|x|Some(x)==child(target)),
            (Self::Delete{source},Call::Delete)=>source.volume()==from&&child(source).is_some(),
            (Self::Restore{deleted_from,target},Call::Restore{..})=>deleted_from.volume()!=from&&target.volume()==to&&
                (child(deleted_from).is_some()||child(target).is_some())&&
                !(deleted_from.volume()!=target.volume()&&child(deleted_from).is_some_and(|x|Some(x)==child(target))),
            _=>false,
        };
        if !ok{return Err(invalid("custody request has wrong ordinary parents, a repeated child or an all-root alias"));}Ok(())
    }
    pub(in crate::ordinary) fn ends(self,call:Call,from:VolumeId,to:VolumeId)->io::Result<(End,End)>{
        self.check(call,from,to)?;
        use End::{Ordinary,Recycle};
        Ok(match(self,call){
            (Self::Roots,Call::Move)=>(Ordinary(Parent::Root(from)),Ordinary(Parent::Root(to))),
            (Self::Roots,Call::Delete)=>(Ordinary(Parent::Root(from)),Recycle(to)),
            (Self::Roots,Call::Restore{..})=>(Recycle(from),Ordinary(Parent::Root(to))),
            (Self::Move{source,target},Call::Move)=>(Ordinary(source),Ordinary(target)),
            (Self::Delete{source},Call::Delete)=>(Ordinary(source),Recycle(to)),
            (Self::Restore{target,..},Call::Restore{..})=>(Recycle(from),Ordinary(target)),
            _=>return Err(invalid("wrong ordinary parent purpose")),
        })
    }
    pub(in crate::ordinary) fn ordinary_move(self,from:VolumeId,to:VolumeId)->io::Result<(Parent,Parent)>{
        match self.ends(Call::Move,from,to)?{(End::Ordinary(a),End::Ordinary(b))=>Ok((a,b)),_=>Err(invalid("two ordinary FILE parents required"))}
    }
    /// Resolve only the ordinary ends touched now, never an offline, historical
    /// delete source. That latter address must come from the exact matching
    /// confirmed deletion, not from a submitted path or a newer location.
    pub(in crate::ordinary) fn bind(self,call:Call,from:VolumeId,to:VolumeId,deleted_at:Option<Parent>,
        mut original:impl FnMut(Parent)->io::Result<Anchor>)->io::Result<Parents>{
        let(l,r)=self.ends(call,from,to)?;
        match(self,call,deleted_at){
            (Self::Roots,Call::Restore{..},Some(Parent::Root(v))) if v!=from=>{},
            (Self::Restore{deleted_from,..},Call::Restore{..},Some(v)) if v==deleted_from=>{},
            (_,Call::Restore{..},_)|(_,_,Some(_))=>return Err(invalid("different original deletion parent or use of a root-only receipt for a child")),
            _=>{},
        }
        let mut anchor=|p:Parent|{let a=original(p)?;if a.address(p.volume())!=p{
            return Err(invalid("ordinary parent decision is not the requested directory"));}Ok(a)};
        Ok(match self{
            Self::Roots=>{for p in [l,r].into_iter().filter_map(End::ordinary){anchor(p)?;}Parents::Roots}
            Self::Move{source,target}=>Parents::Move{source:anchor(source)?,target:anchor(target)?},
            Self::Delete{source}=>Parents::Delete{source:anchor(source)?},
            Self::Restore{deleted_from,target}=>Parents::Restore{deleted_from,target:anchor(target)?},
        })
    }
    pub(in crate::ordinary) fn request(self,call:Call,pool:PoolId,uid:u32,op:OperationId,file:FileId,
        from:VolumeId,to:VolumeId,epoch:u64)->io::Result<[u8;32]>{
        self.check(call,from,to)?;
        let root_domain=call.request(pool,uid,op,file,from,to,epoch)?;
        if self==Self::Roots{return Ok(root_domain);}
        let mut b=b"ZXS/FILE/CUSTODY-AT-ORDINARY-PARENTS/1\0".to_vec();b.extend_from_slice(&root_domain);
        // Root/Child and full ordinary VolumeId are explicit. Restore's
        // original and target are distinct even when they resolve identically.
        match self{
            Self::Move{source,target}=>{b.push(0);source.put(&mut b);target.put(&mut b);}
            Self::Delete{source}=>{b.push(1);source.put(&mut b);}
            Self::Restore{deleted_from,target}=>{b.push(2);deleted_from.put(&mut b);target.put(&mut b);}
            Self::Roots=>unreachable!(),
        }
        crate::hash::digest(&b)
    }
}

fn put_anchor(a:&Anchor,b:&mut Vec<u8>){
    match a{Anchor::Root=>b.push(0),Anchor::Child{..}=>{b.push(1);a.put(b);}}
}
fn get_anchor(r:&mut In<'_>)->io::Result<Anchor>{
    match r.byte()?{0=>Ok(Anchor::Root),1=>Anchor::get(crate::ordinary::file_parent::CHILD,r),
        _=>Err(invalid("unknown ordinary custody anchor"))}
}
fn child_object(a:&Anchor)->Option<&NativeObject>{match a{Anchor::Root=>None,Anchor::Child{object,..}=>Some(object)}}
fn aliases(a:&NativeObject,b:&NativeObject)->bool{a.entity==b.entity||a.birth==b.birth}

impl Parents{
    /// A checked directory's original decision and native identity, not just its
    /// selector, must agree with the corresponding recorded participation.
    pub(in crate::ordinary::custody) fn identifies(&self,source:bool,volume:VolumeId,basis:&crate::ordinary::directory::types::Basis)->bool{
        let a=match(self,source){
            (Self::Roots,_)=>&Anchor::Root,(Self::Move{source,..},true)|(Self::Delete{source},true)=>source,
            (Self::Move{target,..},false)|(Self::Restore{target,..},false)=>target,_=>return false,
        };a.identifies(volume,basis)
    }
    pub(super) fn choice(&self,from:VolumeId,to:VolumeId)->Choice{match self{
        Self::Roots=>Choice::Roots,
        Self::Move{source,target}=>Choice::Move{source:source.address(from),target:target.address(to)},
        Self::Delete{source}=>Choice::Delete{source:source.address(from)},
        Self::Restore{deleted_from,target}=>Choice::Restore{deleted_from:*deleted_from,target:target.address(to)},
    }}
    fn anchors(&self)->(Option<&NativeObject>,Option<&NativeObject>){match self{
        Self::Roots=>(None,None),Self::Move{source,target}=>(child_object(source),child_object(target)),
        Self::Delete{source}=>(child_object(source),None),Self::Restore{target,..}=>(None,child_object(target)),
    }}
    pub(super) fn check(&self,call:Call,from:VolumeId,to:VolumeId,src:&NativeObject,dst:&NativeObject)->io::Result<()>{
        self.choice(from,to).check(call,from,to)?;
        let(a,b)=self.anchors();
        for (anchor,root) in [(a,src),(b,dst)]{
            if anchor.is_some_and(|x|aliases(x,src)||aliases(x,dst)||x.entity.device!=root.entity.device||
                x.birth.filesystem()!=root.birth.filesystem()){
                return Err(invalid("ordinary custody child is not a distinct original on its selected filesystem"));}
        }
        if matches!((a,b),(Some(a),Some(b)) if aliases(a,b)){return Err(invalid("two different ordinary parents required"));}
        Ok(())
    }
    pub(super) fn check_material(&self,s:&Sites)->io::Result<()>{
        let(a,b)=self.anchors();for dir in [a,b].into_iter().flatten(){
            if s.each().any(|role|aliases(dir,role)){return Err(invalid("FILE role cannot also be an ordinary custody directory"));}
        }Ok(())
    }
    pub(super) fn tag(&self,base:u8)->u8{if *self==Self::Roots{base}else{base+4}}
    pub(super) fn split(tag:u8)->io::Result<(u8,bool)>{
        match tag{0..=3=>Ok((tag,false)),4..=7=>Ok((tag-4,true)),_=>Err(invalid("unknown custody method and parent scope"))}
    }
    pub(super) fn put(&self,b:&mut Vec<u8>){match self{
        Self::Roots=>{},Self::Move{source,target}=>{put_anchor(source,b);put_anchor(target,b);}
        Self::Delete{source}=>put_anchor(source,b),
        Self::Restore{deleted_from,target}=>{deleted_from.put(b);put_anchor(target,b);}
    }}
    pub(super) fn get(scoped:bool,call:Call,r:&mut In<'_>)->io::Result<Self>{
        if !scoped{return Ok(Self::Roots);}
        Ok(match call{
            Call::Move=>Self::Move{source:get_anchor(r)?,target:get_anchor(r)?},
            Call::Delete=>Self::Delete{source:get_anchor(r)?},
            Call::Restore{..}=>Self::Restore{deleted_from:Parent::get(r)?,target:get_anchor(r)?},
        })
    }
}
