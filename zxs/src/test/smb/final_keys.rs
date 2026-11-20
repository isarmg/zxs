//! Independent original-wire known answers: Python hashlib SHA-512, HMAC
//! SHA-256 SP800-108 and cryptography AES-CMAC. All inputs are the published
//! synthetic vectors, not a capture, real account or interoperability result.
use super::*;
use crate::smb::connection::sessions::proof::tests::fixture_terminal;

fn hex(s:&str)->Vec<u8> {s.as_bytes().chunks_exact(2).map(|p|
    u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()}
// Chain through actual final request, signing key, distinct application key, complete signed SMB response.
const EXPECT:[[&str;4];2]=[
    [
        "a6d61bd76f3998c1be6a3b1a50a3a480b59e0aec1332bec2a3c45002ecd25509fa46d6562970c21517dc5ae9e2f04586293b582ec474cfef57761ce4f51df95f",
        "90da9577494a46813d0254b5ed28887f",
        "2a4ba00dfe684120057d6174792fb9b7",
        "fe534d424000010000000000010001002900000000000000020000000000000011223344000000000100000000000000b64086adc8a7f9751fd87ee6821ccc100900000048000000",
    ],
    [
        "8b8031429931f94fcbc33c98cb3f64e66159547e7db9c163e6aa18baee39adb65a1a8e1c62c98fc5b4a0c59aae15ff12f0d3fceee4975160f062f9d91eefeb6e",
        "426f2f5fe15c143297e13b48d0bdc3dc",
        "be92e126686a40ae797c6a817d7705fb",
        "fe534d424000010000000000010001002900000000000000020000000000000011223344000000000100000000000000ec2fa021cb382a7d89f816e1ace120900900000048001d00a11b3019a0030a0100a3120410010000001d5393a25f19cafb00000000",
    ],
];
#[test]
fn raw_and_spnego_strict_proofs_sign_real_bytes_with_original_priority_and_distinct_secret_keys() {
    for (i,w) in [false,true].into_iter().enumerate() {
        fixture_terminal(w,|proof,service|{
            let final_msg=proof.seal(service,1).unwrap();let k=&final_msg.bound.keys;
            assert_eq!(k.context.as_bytes().as_slice(),hex(EXPECT[i][0]).as_slice());
            assert_eq!(&k.signer.key[..],hex(EXPECT[i][1]).as_slice());
            assert_eq!(&k.application[..],hex(EXPECT[i][2]).as_slice());
            assert_ne!(&k.signer.key[..],&k.application[..]);
            assert_eq!(&k.session[..],hex("f7bcf45a167d64d4096548daf3c0595a").as_slice());
            assert_eq!(k.signer.id.get(),1);
            assert_eq!(final_msg.bytes,hex(EXPECT[i][3]));
            assert!(final_msg.bound.current(service));
            service.fixture_revoke();assert!(!final_msg.bound.current(service));
        });
    }
}
#[test]
fn a_detached_or_revoked_provider_cannot_seal_the_exchanges_success() {
    fixture_terminal(false,|proof,_|{
        assert!(proof.seal(&ChallengeService::fixture(),1).is_err());
    });
    fixture_terminal(false,|mut proof,original|{
        proof.expires=std::time::Instant::now();assert!(proof.seal(original,1).is_err());
    });
    fixture_terminal(true,|proof,original|{
        original.fixture_revoke();assert!(proof.seal(original,1).is_err());
    });
}
