/* SPDX-License-Identifier: Apache-2.0 */
/* Private native issuer boundary fixtures, not an authority to write media.
 * Link with the pinned Samba tdb_pack and libtdb only when run is authorized.
 */
#define HELPER_ACCOUNT_ISSUER 1
#define main account_readonly_fixture
#include "native-account-snapshot.c"
#undef main

int main(void) {
    assert(account_readonly_fixture() == 0);
    uint8_t sid[68] = {1,4,0,0,0,0,0,5,21,0,0,0,1,0,0,0,2,0,0,0,3};
    struct tdb_context *public = tdb_open(NULL,0,TDB_INTERNAL,O_RDWR,0);
    assert(public != NULL);
    assert(tdb_store(public,machine_key(),bytes(sid,sizeof(sid)),TDB_INSERT)==0);
    assert(tdb_parse_record(public,machine_key(),machine_sid,sid)==0);
    assert(tdb_parse_record(public,bytes(machine_sid_key,sizeof(machine_sid_key)),machine_sid,sid)==-1);
    sid[23]^=1;
    assert(tdb_parse_record(public,machine_key(),machine_sid,sid)==-1);
    assert(tdb_close(public)==0);
    struct helper_native_expected want = {
        .login_len = sizeof(TEST_LOGIN) - 1, .login = TEST_LOGIN,
        .target = ROOT_EPOCH "3132333435363738393a3b3c3d3e3f40:0000000000000001"
    };
    // These are byte sentinels in a synthetic SAMU field, never a real
    // password/account. Use no local disk fixture in the packer tests.
    uint8_t one[16] = {1}, two[16] = {2};
    const uint32_t start = 1700000000, later = 1700000011;
    struct material birth = {.want = &want, .private_nt = one, .last_set = start};
    assert(expectations(&want) && initial(&birth));
    struct change_fields slots;
    assert(samu((struct field){birth.packed, birth.length}, &want, &slots) == HELPER_NATIVE_TARGET);
    assert(birth.packed[slots.nt] == 1);
    assert(birth.packed[slots.description] == 'Z');

    memcpy(want.previous, want.target, 154);
    const char target[] = ROOT_EPOCH "4142434445464748494a4b4c4d4e4f50:0000000000000002";
    memcpy(want.target, target, 154); want.has_previous = true;
    struct material reset = {.want = &want, .private_nt = two, .last_set = later};
    assert(expectations(&want));
    TDB_DATA original = bytes(birth.packed, birth.length);
    assert(replacement(bytes(NULL, 0), original, &reset) == 0 && reset.length == birth.length);
    assert(samu((struct field){reset.packed, reset.length}, &want, &slots) == HELPER_NATIVE_TARGET);
    assert(reset.packed[slots.nt] == 2);
    for (size_t i = 0; i < reset.length; ++i) {
        if ((i >= 16 && i < 20) || (i >= slots.nt && i < slots.nt+16)
            || (i >= slots.description && i < slots.description+154)) continue;
        assert(birth.packed[i] == reset.packed[i]);
    }
    assert(replacement(bytes(NULL, 0), bytes(reset.packed, reset.length), &birth) == -1);
    return 0;
}
