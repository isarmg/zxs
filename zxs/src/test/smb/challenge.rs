//! Source-only public, invented inputs: the same independent reference Type 2
//! as the strict three-message fixture. No operating-system or stored secret.
use super::*;
use crate::smb::session::{der,envelope::tests as frame,strict_verifier::tests::{revision,challenge_vector}};
use std::time::Duration;

fn utf(s:&str)->Box<[u8]> {s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>().into_boxed_slice()}
impl ChallengeService {
    pub(in crate::smb) fn fixture()->Self {
        Self {names:ServerNames{machine:utf("BOX"),nb_domain:utf("SYNTH"),dns_machine:None,
            major:10,minor:0,build:19045},revision:RwLock::new(Some(revision()))}
    }
    pub(in crate::smb) fn fixture_issue(&self,start:HaveNegotiate,id:NonZeroU64,expires:Instant)->Result<ChallengeStep> {
        self.issue_from(start,id,expires,||Ok(([0x11,0x12,0x13,0x14,0x15,0x16,0x17,0x18],0x01da_bc00_1122_3344)))
    }
    pub(in crate::smb) fn fixture_revoke(&self) {*self.revision.write().unwrap()=None;}
}
fn actual(gss:&[u8],mid:u64,sid:u64)->Vec<u8> {
    let mut p=frame::smb(gss,true,mid,sid,0);p.truncate(72);
    p[68..70].copy_from_slice(&72u16.to_le_bytes());p.extend_from_slice(gss);p
}
fn issue(s:&ChallengeService,first:&[u8])->ChallengeStep {
    s.fixture_issue(frame::seen(first),NonZeroU64::new(42).unwrap(),Instant::now()+Duration::from_secs(30)).unwrap()
}

#[test]
fn fixed_standalone_raw_challenge_exact_name_flags_nonce_version_av_and_absolute_expiry() {
    let s=ChallengeService::fixture();let (one,value)=challenge_vector();
    let step=issue(&s,&one);assert_eq!(step.token(),value.as_slice());
    let p=actual(step.token(),1,42);let pending=step.bind(&p).unwrap();
    assert!(pending.current_at(&s,Instant::now()));assert!(!pending.current_at(&s,pending.expires));
    assert!(!pending.current_at(&ChallengeService::fixture(),Instant::now()));
    s.fixture_revoke();assert!(!pending.current_at(&s,Instant::now()));
    let live=ChallengeService::fixture();
    assert!(live.fixture_issue(frame::seen(&one),NonZeroU64::MAX,Instant::now()+Duration::from_secs(30)).is_err());
    assert!(live.fixture_issue(frame::seen(&one),NonZeroU64::new(42).unwrap(),Instant::now()).is_err());
    assert!(live.issue_from(frame::seen(&one),NonZeroU64::new(42).unwrap(),Instant::now()+Duration::from_secs(30),||Err(Rejected)).is_err());
}

#[test]
fn wrapped_challenge_never_drops_first_request_mic_and_never_repeats_supported_oid() {
    let s=ChallengeService::fixture();let (one,two)=challenge_vector();
    let optimistic=frame::init(&[frame::NTLM,frame::KRB],Some(&one));
    let step=issue(&s,&optimistic);
    assert_eq!(step.token(),frame::resp(Some(3),true,Some(&two),None).as_slice());
    // Same exact Type 2 inside weaker remembered first envelope is not the
    // generated reply; do not mistake equal NTLM octets for equal SMB/GSS.
    assert!(step.bind(&actual(&frame::resp(Some(1),true,Some(&two),None),1,42)).is_err());
    let later=wrapped(ChallengeLayer::LaterSpnego,&two).unwrap();
    assert_eq!(&later,&frame::resp(Some(1),false,Some(&two),None));
    let step=issue(&s,&optimistic);let p=actual(step.token(),1,42);assert!(step.bind(&p).is_ok());
    for n in [127usize,128,255,256,590] {
        let payload=vec![0x77;n];let outer=wrapped(ChallengeLayer::FirstSpnego,&payload).unwrap();
        let parsed=der::subsequent(&outer).unwrap();
        assert_eq!(parsed.state,Some(3));assert!(parsed.selection);assert_eq!(parsed.token,Some(&payload[..]));
    }
}

#[test]
fn returns_only_supported_connection_flags_and_trusted_bounded_utf16() {
    let s=ChallengeService::fixture();let minimum=MIN|ALWAYS_SIGN;
    let server=TARGET_INFO|0x0002_0000|4;
    assert_eq!(choose(minimum).unwrap(),minimum|server);
    assert_eq!(choose(minimum|0x80|2).unwrap(),minimum|server); // ESS not LM_KEY; UNICODE not OEM
    for bit in [super::super::SEAL,super::super::KEY_EXCH,super::super::BITS56] {
        assert_eq!(choose(minimum|bit).unwrap(),minimum|server|bit);
    }
    for bit in [ALWAYS_SIGN,super::super::SIGN,super::super::ESS,super::super::BITS128] {assert!(choose(minimum&!bit).is_err());}
    for bit in [0x40,0x800,0x0010_0000,0x0040_0000] {assert!(choose(minimum|bit).is_err());}
    for v in [&[][..],&[0][..],&[0,0][..],&[0,0xd8][..],&[65;32][..]] {assert!(name(v,30).is_err());}
    let mut names=s.names;names.dns_machine=Some(utf("box.example.invalid"));
    let encoded=names.encode(choose(minimum).unwrap(),[4;8],1).unwrap();
    let descriptor=crate::smb::session::ntlm_shape::challenge_fields(&encoded).unwrap();
    assert_eq!(descriptor.avs.get(3),names.dns_machine.as_deref());
}
