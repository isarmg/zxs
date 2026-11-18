use super::*;
use crate::{OperationId,initialize,test_support::PrivateTemp};
use std::{fs as stdfs,path::Path};

#[test]
#[ignore="requires a separately mounted writable test filesystem supporting native handles and user xattrs"]
fn original_route_is_purpose_limited_and_retained_root_cannot_be_lent_to_ordinary_calls(){
    let a=PrivateTemp::under(&std::env::temp_dir());
    let b=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    assert_ne!(a.1.device,b.1.device);
    let sys=a.child("system");let only=a.child("ordinary");let extra=a.child("unrelated");let r=b.child("recycle");
    let info=initialize(OperationId::new().unwrap(),&sys,&r,&[only.clone(),extra.clone()]).unwrap();
    let id=|p:&Path|info.volumes.iter().find(|v|v.volume.path.as_path()==p).unwrap().id;
    let (vol,sysid,bin)=(id(&only),id(&sys),id(&r));
    let p=LocalPool::open(&sys,Access::Read).unwrap();
    for(direction,from,to)in[(Direction::ToRecycle,vol,bin),(Direction::FromRecycle,bin,vol)]{
        let held=p.observe_dedicated(direction,from,to).unwrap();
        let(f,t)=held.registration().originals();
        assert_eq!((f.0,t.0),(from,to));assert_ne!(f.1.device,t.1.device);assert_ne!(f.2.filesystem(),t.2.filesystem());
        assert!(held.registration().matches(f,t));assert!(!held.registration().matches(t,f));
        assert!(p.registration.dedicated_route(fs::local_uid()^1,direction,from,to).is_err());
        assert!(p.observe_dedicated(direction,to,from).is_err());
    }
    for(from,to)in[(vol,sysid),(sysid,bin),(vol,vol),(vol,id(&extra)),(vol,VolumeId::new().unwrap())]{
        assert!(p.observe_dedicated(Direction::ToRecycle,from,to).is_err());
    }
    assert!(p.ordinary(bin).is_err());assert!(p.ordinary(sysid).is_err());
    stdfs::rename(&extra,a.0.join("offline-unrelated")).unwrap();
    let held=p.observe_dedicated(Direction::ToRecycle,vol,bin).unwrap();
    let original_marker=held.recycle.label().unwrap().unwrap();
    let old=b.0.join("disconnected-original");stdfs::rename(&r,&old).unwrap();
    assert!(held.require_current().is_err());
    assert!(p.observe_dedicated(Direction::ToRecycle,vol,bin).is_err());
    assert!(p.ordinary(vol).is_ok());
    // Same original label, same filesystem, same path, different physical root.
    let replacement=b.child("recycle");
    HeldDir::open(&replacement).unwrap().claim(&original_marker).unwrap();
    assert!(held.require_current().is_err());
    assert!(p.observe_dedicated(Direction::FromRecycle,bin,vol).is_err());
    assert_eq!(stdfs::read_dir(&replacement).unwrap().count(),0);
    assert_eq!(stdfs::read_dir(&old).unwrap().count(),0);
    assert!(p.ordinary(vol).is_ok());
}
