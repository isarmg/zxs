/* SPDX-License-Identifier: Apache-2.0 */
/* Source fixture for the exact libtdb build. Uses TDB_INTERNAL, never a
 * credentials file or media. No classified result here can enable access.
 */
#include "../../resources/samba-native/account-snapshot.c"
#include <assert.h>

#define TEST_LOGIN "smb.name"
#define EXAMPLE_POOL "0102030405060708090a0b0c0d0e0f10"
#define EXAMPLE_SITE "1112131415161718191a1b1c1d1e1f20"
#define EXAMPLE_SMB  "2122232425262728292a2b2c2d2e2f30"
#define ROOT_EPOCH "ZXS1:" EXAMPLE_POOL ":" EXAMPLE_SITE ":" EXAMPLE_SMB ":"

struct pack { uint8_t bytes[1024]; size_t head; };
static void narrow(struct pack *p, unsigned n, uint32_t v) {
    for (unsigned i = 0; i < n; ++i) p->bytes[p->head++] = (uint8_t)(v >> (8 * i));
}
static void blob(struct pack *p, const void *bytes, size_t n) {
    assert(p->head + 4 + n <= sizeof(p->bytes));
    narrow(p, 4, n);
    if (n != 0) memcpy(p->bytes + p->head, bytes, n);
    p->head += n;
}
static struct pack sample(const uint8_t *marker, size_t width, uint32_t flags, bool lm, bool history) {
    struct pack p = {0};
    uint8_t hours[32] = {0};
    memset(hours, 255, 21);
    uint8_t placeholders[32] = {7}; /* length fixtures, not password checks */
    narrow(&p, 4, 0); /* last logon */
    narrow(&p, 4, 0x7fffffff); narrow(&p, 4, 0x7fffffff); /* logoff and kickoff */
    narrow(&p, 4, 0); narrow(&p, 4, 1700000000); /* last bad and last set */
    narrow(&p, 4, 0); narrow(&p, 4, 0); /* policy dates as stored, not derived */
    blob(&p, TEST_LOGIN, sizeof(TEST_LOGIN));
    blob(&p, "ZXSLOCAL", sizeof("ZXSLOCAL"));
    for (int i = 0; i < 6; ++i) blob(&p, NULL, 0);
    blob(&p, marker, width);
    for (int i = 0; i < 3; ++i) blob(&p, NULL, 0);
    narrow(&p, 4, 1000); narrow(&p, 4, 513);
    blob(&p, placeholders, lm ? 16 : 0);
    blob(&p, placeholders, 16);
    blob(&p, placeholders, history ? 32 : 0);
    narrow(&p, 4, flags);
    narrow(&p, 2, 168); narrow(&p, 4, 21); blob(&p, hours, sizeof(hours));
    narrow(&p, 2, 0); narrow(&p, 2, 0); narrow(&p, 4, 0x4ec);
    return p;
}
static TDB_DATA data(const void *p, size_t n) { return (TDB_DATA) { (unsigned char *)p, n }; }
static void put(struct tdb_context *db, const char *key, const void *p, size_t n, int flag) {
    assert(tdb_store(db, data(key, strlen(key) + 1), data(p, n), flag) == 0);
}
static void remove_key(struct tdb_context *db, const char *key) {
    assert(tdb_delete(db, data(key, strlen(key) + 1)) == 0);
}
int main(void) {
    const struct helper_native_expected want = {
        .login_len = sizeof(TEST_LOGIN) - 1, .login = TEST_LOGIN, .has_previous = true,
        .previous = ROOT_EPOCH "3132333435363738393a3b3c3d3e3f40:0000000000000001",
        .target = ROOT_EPOCH "4142434445464748494a4b4c4d4e4f50:0000000000000002"
    };
    const uint8_t v4[4] = {4}, v0[4] = {0};
    struct tdb_context *db = tdb_open(NULL, 0, TDB_INTERNAL, O_RDWR, 0);
    assert(db != NULL);
    struct pack original = sample(want.previous, 154, 0x10, false, false);
    put(db, "INFO/version", v4, sizeof(v4), TDB_INSERT);
    put(db, "INFO/minor_version", v0, sizeof(v0), TDB_INSERT);
    put(db, "USER_" TEST_LOGIN, original.bytes, original.head, TDB_INSERT);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED); /* isolated USER */
    put(db, "RID_000003e8", TEST_LOGIN, sizeof(TEST_LOGIN), TDB_INSERT);
    assert(inspect(db, &want) == HELPER_NATIVE_PREVIOUS);
    put(db, "USER_other", original.bytes, original.head, TDB_INSERT);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED); /* orphan, not seen by search_users */
    remove_key(db, "USER_other");
    put(db, "RID_000003e8", "other", sizeof("other"), TDB_MODIFY);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED);
    put(db, "RID_000003e8", TEST_LOGIN, sizeof(TEST_LOGIN), TDB_MODIFY);
    put(db, "INFO/minor_version", v4, sizeof(v4), TDB_MODIFY);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED);
    put(db, "INFO/minor_version", v0, sizeof(v0), TDB_MODIFY);

    struct traversal inventory = {.want = &want};
    assert(stored_key(data("USER_wrong", sizeof("USER_wrong")),
        data(original.bytes, original.head), &inventory) == -1);
    assert(inventory.failed); /* physical callback rejects, even absent a linked traversal */

    uint8_t suffix[155];
    memcpy(suffix, want.target, 154); suffix[154] = 'x';
    struct pack rows[] = {
        sample(suffix, sizeof(suffix), 0x10, false, false),
        sample(want.target, 154, 0x11, false, false),
        sample(want.target, 154, 0x10, true, false),
        sample(want.target, 154, 0x10, false, true),
    };
    for (size_t i = 0; i < sizeof(rows)/sizeof(rows[0]); ++i) {
        put(db, "USER_" TEST_LOGIN, rows[i].bytes, rows[i].head, TDB_MODIFY);
        assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED);
    }
    struct pack newer = sample(want.target, 154, 0x10, false, false);
    struct pack never_set = newer;
    memset(never_set.bytes + 16, 0, 4);
    put(db, "USER_" TEST_LOGIN, never_set.bytes, never_set.head, TDB_MODIFY);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED);
    struct pack expiring = newer;
    memset(expiring.bytes + 8, 0, 4); expiring.bytes[8] = 1;
    put(db, "USER_" TEST_LOGIN, expiring.bytes, expiring.head, TDB_MODIFY);
    assert(inspect(db, &want) == HELPER_NATIVE_UNCONFIRMED);
    put(db, "USER_" TEST_LOGIN, newer.bytes, newer.head, TDB_MODIFY);
    assert(inspect(db, &want) == HELPER_NATIVE_TARGET);
    assert(tdb_close(db) == 0);
    return 0;
}
