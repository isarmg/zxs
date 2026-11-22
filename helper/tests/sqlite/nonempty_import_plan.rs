use super::*;

const SRC:[u64;3]=[4,5,6];
const DEST:[u64;3]=[7,8,9];
const VOLUME:&str="77777777777777777777777777777777";
const ARGS:[&str;14]=["--init-actor","03030303030303030303030303030303","--storage-uid","1001",
    "--storage-gid","1002","--system","/original/S","--pool","01010101010101010101010101010101",
    "--init","02020202020202020202020202020202","--source","/fixture/source"];
fn selection(extra:&[&str])->impl Iterator<Item=OsString>{ARGS.into_iter().chain(extra.iter().copied()).map(OsString::from)}

#[test]
fn a_plan_needs_both_independent_operands_and_no_action(){
    assert!(ImportCheck::parse_plan(selection(&[])).is_err());
    assert!(ImportCheck::parse_plan(selection(&["--target","/media/F"])).is_err());
    assert!(ImportCheck::parse_plan(selection(&["--target-volume",VOLUME])).is_err());
    for path in ["/fixture/source/inside","/fixture","/original/S","/original/S/inside"]{
        assert!(ImportCheck::parse_plan(selection(&["--target-volume",VOLUME,"--target",path])).is_err());
    }
    let plan=["--target-volume",VOLUME,"--target","/media/F"];
    assert!(ImportCheck::parse(selection(&plan)).is_err());
    let q=ImportCheck::parse_plan(selection(&plan)).unwrap();
    for flag in ["--force","--apply","--output","--resume"]{
        assert!(ImportCheck::parse_plan(selection(&plan).chain([OsString::from(flag)])).is_err());
    }
    let claim=q.target.unwrap();assert_eq!(claim.volume,[0x77;16]);assert_eq!(claim.path,Path::new("/media/F"));
}

#[test]
fn target_extension_is_exact_and_does_not_change_the_inventory_prefix(){
    let old=ImportCheck::parse(selection(&[])).unwrap();
    let plan=ImportCheck::parse_plan(selection(&["--target-volume",VOLUME,"--target","/media/F"])).unwrap();
    let head=encode(&old,&[7;32],[1,2,3],SRC,None).unwrap();
    let full=encode(&plan,&[7;32],[1,2,3],SRC,Some(DEST)).unwrap();
    assert_eq!(&full[..head.len()],head);
    let read=Question::decode(&full).unwrap();
    assert_eq!(read.target.unwrap(),DEST);
    assert_eq!(read.q.target.unwrap().path,Path::new("/media/F"));
    assert!(Question::decode(&full[..full.len()-1]).is_err());
    assert!(Question::decode(&[full.as_slice(),&[0]].concat()).is_err());
    assert!(encode(&plan,&[7;32],[1,2,3],SRC,None).is_err());
    assert!(encode(&old,&[7;32],[1,2,3],SRC,Some(DEST)).is_err());
    assert!(encode(&plan,&[7;32],[1,2,3],SRC,Some([7,8,0])).is_err());
}

#[test]
fn plan_evidence_is_bound_to_both_original_mounts_and_destination_id(){
    let [pool,init,volume]=[[1;16],[2;16],[0x77;16]];
    let facts=TargetFacts{available:100,limit:255,blockers:0};
    let counts=Counts{files:2,dirs:1,empty_dirs:1,logical_bytes:20};
    let plan=Reply::Planned(counts,facts,volume,6,9);
    let raw=plan.encode(pool,init);
    assert_eq!(Reply::decode(&raw,pool,init,6,Some((volume,9))).unwrap(),plan);
    assert!(Reply::decode(&raw,pool,init,6,None).is_err());
    assert!(Reply::decode(&raw,pool,init,5,Some((volume,9))).is_err());
    assert!(Reply::decode(&raw,pool,init,6,Some((volume,10))).is_err());
    assert!(Reply::decode(&raw,pool,init,6,Some(([2;16],9))).is_err());
    for field in [1,2]{
        let mut bad=raw;bad[field]=if field==1{0x80}else{0};
        assert!(Reply::decode(&bad,pool,init,6,Some((volume,9))).is_err());
    }
    let bare=Reply::Inventoried(counts,6).encode(pool,init);
    assert!(Reply::decode(&bare,pool,init,6,Some((volume,9))).is_err());
    let (status,text)=plan.render(pool,init,Some(volume));
    assert_eq!(status,0);assert!(text.ends_with(PLAN_ONLY));
    let short=Reply::Planned(counts,TargetFacts{available:19,..facts},volume,6,9);
    assert_eq!(short.render(pool,init,Some(volume)).0,1);
    let readonly=Reply::Planned(counts,TargetFacts{blockers:import_source::READ_ONLY,..facts},volume,6,9);
    assert_eq!(readonly.render(pool,init,Some(volume)).0,1);
}
