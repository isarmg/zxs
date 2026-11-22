use super::*;
const VECTOR:&str=include_str!("../../../../sqlite/shared-account-public-v1.txt");
fn value(key:&str)->&'static str{
    VECTOR.lines().filter_map(|s|s.split_once('=')).find(|(k,_)|*k==key).unwrap().1
}
fn hex<const N:usize>(key:&str)->[u8;N]{
    let text=value(key);
    assert_eq!(text.len(),2*N);
    std::array::from_fn(|i|u8::from_str_radix(&text[2*i..2*i+2],16).unwrap())
}
fn scope()->SharedScope<'static>{
    SharedScope{pool:hex("pool"),init:hex("init"),setup:hex("setup"),instance:hex("instance"),
        bind:hex("bind"),account:hex("account"),original_actor:hex("actor"),init_sha:hex("init_sha"),
        plan_sha:hex("plan_sha"),login:value("login").as_bytes(),sid:hex("sid")}
}
fn epochs()->(PublicEpoch,PublicEpoch){
    (PublicEpoch{op:hex("setup"),generation:1,record:hex("first_record")},
     PublicEpoch{op:hex("new_op"),generation:2,record:hex("new_record")})
}
#[test]
fn shared_account_public_vector_matches_external_digest_and_fixed_vault_prefix(){
    let s=scope();let keys=s.role_keys().unwrap();let (old,new)=epochs();
    assert_eq!(keys.web,hex("web_actor"));assert_eq!(keys.smb,hex("smb_actor"));
    assert_eq!(s.setup_request(hex("setup_approval"),old.record).unwrap(),hex("setup_sha"));
    assert_eq!(s.reset_request(hex("reset_approval"),old,new).unwrap(),hex("reset_sha"));
    assert_eq!(s.vault_prefix(old).unwrap(),hex("first_prefix"));
    assert_eq!(s.vault_prefix(new).unwrap(),hex("new_prefix"));
}
#[test]
fn shared_account_public_rejects_stale_forks_aliases_and_original_role_collisions(){
    let s=scope();let (old,new)=epochs();let approval=hex("reset_approval");
    for wrong in [
        SharedScope{login:b"guest",..s},SharedScope{login:b"Shared.operator",..s},
        SharedScope{login:b"a\0b",..s},SharedScope{login:&[b'a';65],..s},
        SharedScope{original_actor:s.account,..s},
        SharedScope{original_actor:s.role_keys().unwrap().smb,..s},
        SharedScope{init_sha:[0;32],..s},SharedScope{plan_sha:[0;32],..s},
        SharedScope{sid:[0;28],..s},SharedScope{bind:s.instance,..s},SharedScope{pool:s.setup,..s},
    ]{
        assert!(wrong.reset_request(approval,old,new).is_err());
        assert!(wrong.vault_prefix(new).is_err());
    }
    for wrong in [
        PublicEpoch{generation:1,..new},PublicEpoch{generation:3,..new},
        PublicEpoch{op:old.op,..new},PublicEpoch{op:s.init,..new},
        PublicEpoch{record:old.record,..new},PublicEpoch{record:[0;16],..new},
    ]{assert!(s.reset_request(approval,old,wrong).is_err());}
    for alias in s.installation(){
        let wrong=PublicEpoch{op:alias,..new};
        assert!(s.reset_request(approval,old,wrong).is_err());
        assert!(s.vault_prefix(wrong).is_err());
    }
    assert!(s.reset_request([0;16],old,new).is_err());
    assert!(s.reset_request(new.op,old,new).is_err());
    assert!(s.setup_request(s.setup,old.record).is_err());
    let actual=s.reset_request(approval,old,new).unwrap();
    for changed in [
        SharedScope{plan_sha:[44;32],..s},SharedScope{init_sha:[57;32],..s},
        SharedScope{login:b"shared.other",..s},SharedScope{account:[60;16],..s},
    ]{assert_ne!(changed.reset_request(approval,old,new).unwrap(),actual);}
}
