/* SPDX-License-Identifier: Apache-2.0 */
/* Private to the future signed, single-purpose maintenance process. A caller
 * must already own the host grant and the exact durable SQL Pending, choose
 * the originally protected mounted directory and ensure both original EX
 * locks and process protections. Nothing here can issue or acquire authority.
 */
#include <dirent.h>
#include <errno.h>
#include <stdlib.h>
#include <time.h>

/* Fixed Samba source3/lib/util_tdb.c; the issuer must link that exact symbol
 * from the independent, pinned Samba source build. No generic foreign ABI. */
extern size_t tdb_pack(uint8_t *buf, int size, const char *format, ...);

#define SAMU_V4 "dddddddBBBBBBBBBBBBddBBBdwdBwwd"
#define ROW_CAPACITY 1024
#define MUTATE_RESULT_FAIL (-1)
#define MUTATE_RESULT_CHANGED 0

struct material {
    const struct helper_native_expected *want;
    const uint8_t *private_nt;
    uint32_t last_set;
    uint8_t packed[ROW_CAPACITY];
    size_t length;
};

static bool checked_now(uint32_t *out) {
    time_t now = time(NULL);
    if (now <= 0 || (uintmax_t)now > UINT32_MAX
        || (uint32_t)now == SAMU_NEVER_EXPIRES || (uint32_t)now == SAMU_MIN_TIME)
        return false;
    *out = (uint32_t)now;
    return true;
}

static bool initial(struct material *m) {
    char name[65] = {0}, domain[] = "ZXSLOCAL";
    memcpy(name, m->want->login, m->want->login_len);
    uint8_t hours[32] = {0};
    memset(hours, 255, 21);
    char *nil = NULL;
    // Seven d timestamps; twelve B strings (including acct_desc); then the
    // two fixed RIDs, three B credentials, and the Samba normal-user trailer.
    m->length = tdb_pack(m->packed, (int)sizeof(m->packed), SAMU_V4,
        UINT32_C(0), SAMU_NEVER_EXPIRES, SAMU_NEVER_EXPIRES, UINT32_C(0), m->last_set, UINT32_C(0), SAMU_NEVER_EXPIRES,
        (int)m->want->login_len + 1, name, (int)sizeof(domain), domain,
        0, nil, 0, nil, 0, nil, 0, nil, 0, nil, 0, nil,
        154, (char *)m->want->target, 0, nil, 0, nil, 0, nil,
        USER_RID, ORDINARY_GROUP_RID, 0, nil, 16, (char *)m->private_nt, 0, nil,
        ACB_NORMAL_ONLY, 168, UINT32_C(21), (int)sizeof(hours), (char *)hours,
        0, 0, UINT32_C(0x4ec));
    return m->length <= sizeof(m->packed)
        && samu((struct field){m->packed, m->length}, m->want, NULL) == HELPER_NATIVE_TARGET;
}

static int replacement(TDB_DATA key, TDB_DATA data, void *arg) {
    (void)key;
    struct material *m = arg;
    struct change_fields fields;
    if (data.dsize > sizeof(m->packed)
        || samu(tdb_field(data), m->want, &fields) != HELPER_NATIVE_PREVIOUS) return -1;
    // Keep every other stored field from this exact account. No LM fields or
    // history exist. This temporary private row is wiped on all exits.
    m->length = data.dsize;
    memcpy(m->packed, data.dptr, m->length);
    memcpy(m->packed + fields.description, m->want->target, 154);
    memcpy(m->packed + fields.nt, m->private_nt, 16);
    for (size_t k = 0; k < 4; ++k) m->packed[16 + k] = (uint8_t)(m->last_set >> (8 * k));
    return samu((struct field){m->packed, m->length}, m->want, NULL) == HELPER_NATIVE_TARGET ? 0 : -1;
}

static TDB_DATA bytes(const void *ptr, size_t length) {
    return (TDB_DATA){(unsigned char *)ptr, length};
}

static int nonempty(TDB_DATA key, TDB_DATA data, void *ignore) {
    (void)key; (void)data; (void)ignore;
    return -1;
}
static int nonempty_chain(struct tdb_context *db, TDB_DATA key, TDB_DATA data, void *ignore) {
    (void)db; (void)key; (void)data; (void)ignore;
    return 1;
}

static bool transaction(struct tdb_context *db, struct material *m, bool new_file) {
    char key[5 + sizeof(m->want->login) + 1] = "USER_";
    TDB_DATA username = bytes(key, 5 + m->want->login_len + 1);
    const uint8_t major[4] = {4}, minor[4] = {0};
    const unsigned forbidden = TDB_NOLOCK | TDB_CLEAR_IF_FIRST | TDB_NOSYNC | TDB_INTERNAL;
    memcpy(key + 5, m->want->login, m->want->login_len);
    if ((tdb_get_flags(db) & forbidden) != 0 || tdb_transaction_start_nonblock(db) != 0) return false;
    if (new_file) {
        if (tdb_check(db, nonempty, NULL) != 0 || tdb_traverse_read(db, nonempty_chain, NULL) != 0
            || !initial(m)
            || tdb_store(db, bytes("INFO/version", sizeof("INFO/version")), bytes(major, sizeof(major)), TDB_INSERT) != 0
            || tdb_store(db, bytes("INFO/minor_version", sizeof("INFO/minor_version")), bytes(minor, sizeof(minor)), TDB_INSERT) != 0
            || tdb_store(db, bytes("RID_000003e8", sizeof("RID_000003e8")), bytes(key + 5, m->want->login_len + 1), TDB_INSERT) != 0)
            goto cancel;
    } else if (inspect(db, m->want) != HELPER_NATIVE_PREVIOUS
               || tdb_parse_record(db, username, replacement, m) != 0) {
        goto cancel;
    }
    if (tdb_store(db, username, bytes(m->packed, m->length), new_file ? TDB_INSERT : TDB_MODIFY) != 0
        || inspect(db, m->want) != HELPER_NATIVE_TARGET)
        goto cancel;
    explicit_bzero(m->packed, sizeof(m->packed));
    // An error can be a late commit result. Never call the action again or
    // turn that return into proof of rollback. Inspect the original only.
    return tdb_transaction_commit(db) == 0;
cancel:
    explicit_bzero(m->packed, sizeof(m->packed));
    (void)tdb_transaction_cancel(db);
    return false;
}

// Enforce both the already-held parent and its fixed raw leaf through the
// supervisor's original view. A renamed object is not a candidate to recover.
static bool dir_place(int site, const struct statx *p, int parent_pin, struct statx *out) {
    int fd = account_directory(site);
    if (fd < 0) return false;
    struct statx current;
    bool ok = stamp(parent_pin, out, true, false) && same_mount(out,p) && out->stx_ino == p->stx_ino
        && stamp(fd, &current, true, false) && same_mount(out,&current) && out->stx_ino == current.stx_ino;
    close(fd);
    return ok;
}

static bool inventory(int parent_pin, const struct statx *p, bool requires_store) {
    int dir = openat(parent_pin, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_NOATIME | O_CLOEXEC);
    if (dir < 0) return false;
    if (!quiet(dir, p, true, false)) { close(dir); return false; }
    DIR *stream = fdopendir(dir);
    if (stream == NULL) { close(dir); return false; }
    bool seen = false, ok = false;
    unsigned left = 3; /* ".", "..", and at most the one expected native name */
    for (;;) {
        errno = 0;
        struct dirent *entry = readdir(stream);
        if (entry == NULL) {
            ok = errno == 0 && seen == requires_store && quiet(dir, p, true, false);
            break;
        }
        if (left == 0) break;
        --left;
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
        if (requires_store && !seen && strcmp(entry->d_name, ACCOUNT_FILE) == 0) { seen = true; continue; }
        break;
    }
    return closedir(stream) == 0 && ok && quiet(parent_pin, p, true, false);
}

static bool pinned(int fd) {
    if (fd < 3) return false;
    int mode = fcntl(fd, F_GETFL);
    return mode != -1 && (mode & O_PATH) != 0;
}

/*
 * A first issue uses only this process's successful O_EXCL, not a reusable
 * "empty" verdict. A reset names the already-verified predecessor's exact
 * O_PATH. Opening this RW handle is allowed only after a separate successful
 * predecessor observation, not as a way to discover or repair unknown media.
 * The approved writer never calls this for an observation or cold target.
 */
int helper_native_change(int site, int parent_pin, int previous_pin, const struct helper_native_expected *want,
    const uint8_t private_nt[16], int *original_out) {
    if (original_out == NULL) return MUTATE_RESULT_FAIL;
    *original_out = -1;
    struct material m = {.want = want, .private_nt = private_nt};
    struct statx p, f, again, actual;
    char alias[64], path[80];
    bool fresh = previous_pin == -1;
    const unsigned mode = O_RDWR | O_CLOEXEC | O_NOATIME | O_NONBLOCK;
    if (site < 3 || private_nt == NULL || previous_pin < -1 || getenv("TDB_NO_FSYNC") != NULL
        || !expectations(want) || fresh == want->has_previous
        || !checked_now(&m.last_set) || !pinned(site) || !pinned(parent_pin) || !stamp(parent_pin, &p, true, false)
        || !dir_place(site, &p, parent_pin, &again)
        || !inventory(parent_pin, &p, !fresh)) return MUTATE_RESULT_FAIL;
    unsigned open_flags = mode;
    const char *input = alias;
    if (fresh) {
        // The exact new basename is relative to OUR retained original parent.
        if (!original_proc_path(parent_pin, &p, true, false, alias)) return MUTATE_RESULT_FAIL;
        int n = snprintf(path, sizeof(path), "%s/" ACCOUNT_FILE, alias);
        if (n < 1 || n >= (int)sizeof(path)) return MUTATE_RESULT_FAIL;
        open_flags |= O_CREAT | O_EXCL | O_NOFOLLOW;
        input = path;
    } else if (!pinned(previous_pin) || !stamp(previous_pin, &f, false, false)
        || !same_mount(&p, &f) || p.stx_ino == f.stx_ino || !names(site, &p, &f, false)
        || observe_mount(site, parent_pin, previous_pin, want, false) != HELPER_NATIVE_PREVIOUS
        || !quiet(previous_pin, &f, false, false)
        || !original_proc_path(previous_pin, &f, false, false, alias)) {
        return MUTATE_RESULT_FAIL;
    }

    struct tdb_context *db = tdb_open_ex(input, 0, TDB_NOMMAP | TDB_DISALLOW_NESTING,
        (int)open_flags, 0600, NULL, NULL);
    if (db == NULL) return MUTATE_RESULT_FAIL; // preserve an uncertain fresh name
    int db_fd = tdb_fd(db), flags = fcntl(db_fd, F_GETFL);
    int pin = -1;
    if (flags != -1 && (flags & O_ACCMODE) == O_RDWR && (flags & O_NOATIME)
        && stamp(db_fd, &actual, false, false) && same_mount(&p, &actual)
        && (fresh || (same_mount(&f, &actual) && f.stx_ino == actual.stx_ino))
        && dir_place(site, &p, parent_pin, &again) && names(site, &again, &actual, false)
        && transaction(db, &m, fresh)
        && stamp(db_fd, &actual, false, false)
        && dir_place(site, &p, parent_pin, &again) && names(site, &again, &actual, false)
        && inventory(parent_pin, &again, true)) {
        pin = openat(parent_pin, ACCOUNT_FILE, O_PATH | O_NOFOLLOW | O_CLOEXEC);
        if (pin >= 0 && !quiet(pin, &actual, false, false)) { close(pin); pin = -1; }
    }
    explicit_bzero(m.packed, sizeof(m.packed));
    // Not a publication: the actual supervisor must wait the worker, obtain
    // the same original inode, sync it and the original parent, then perform
    // the independent read-only verification before changing SQL to ACTIVE.
    int closed = tdb_close(db);
    if (closed != 0 || pin < 0 || !dir_place(site, &p, parent_pin, &again)
        || !inventory(parent_pin, &again, true) || !names(site, &again, &actual, false)
        || !quiet(pin, &actual, false, false)) {
        if (pin >= 0) close(pin);
        return MUTATE_RESULT_FAIL;
    }
    *original_out = pin; /* keep the exact object until the protected supervisor takes it */
    return MUTATE_RESULT_CHANGED;
}

static bool sync_original_fd(int fd) {
    int result;
    do { result = fsync(fd); } while (result != 0 && errno == EINTR);
    return result == 0;
}

/* One approved cold-target completion (or the exact just-written same file).
 * Nothing here tries a native recovery on an unrecognizable/hot database,
 * accepts a predecessor, compares a password, or writes another credential.
 */
bool helper_native_confirm_target(int site, int parent_pin, int store_pin, const struct helper_native_expected *want) {
    struct statx p, f;
    char file_alias[64];
    if (!pinned(site) || !expectations(want) || !pinned(parent_pin) || !pinned(store_pin)
        || !stamp(parent_pin, &p, true, false) || !stamp(store_pin, &f, false, false)
        || !same_mount(&p, &f) || p.stx_ino == f.stx_ino
        || !inventory(parent_pin, &p, true) || !names(site, &p, &f, false)
        || observe_mount(site, parent_pin, store_pin, want, false) != HELPER_NATIVE_TARGET
        || !quiet(parent_pin, &p, true, false) || !quiet(store_pin, &f, false, false)
        || !original_proc_path(store_pin, &f, false, false, file_alias))
        return false;
    // No native handle remains open. Obtain only raw descriptors for fsync,
    // from the same pins, never data-open an uncertain guessed name.
    int dir = openat(parent_pin, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_NOATIME | O_CLOEXEC);
    if (dir < 0) return false;
    int fdb = open(file_alias, O_RDONLY | O_NOATIME | O_NONBLOCK | O_CLOEXEC);
    bool synced = fdb >= 0 && quiet(fdb, &f, false, false) && quiet(dir, &p, true, false)
        && names(site, &p, &f, false) && sync_original_fd(fdb) && sync_original_fd(dir)
        && quiet(fdb, &f, false, false) && quiet(dir, &p, true, false) && names(site, &p, &f, false);
    if (fdb >= 0 && close(fdb) != 0) synced = false;
    if (close(dir) != 0) synced = false;
    // Independently re-open from the held original using genuine libtdb
    // once all raw sync descriptors are closed. Post-sync observation never
    // uses the first query or the writer's cached native context.
    return synced && observe_mount(site, parent_pin, store_pin, want, false) == HELPER_NATIVE_TARGET
        && quiet(store_pin, &f, false, false) && inventory(parent_pin, &p, true) && names(site, &p, &f, false);
}

enum helper_native_match helper_native_predecessor(int site, int parent_pin, int store_pin,
    const struct helper_native_expected *want) {
    /* The host may be mounted RW, yet the native preflight is always O_RDONLY. */
    if (!pinned(site) || !pinned(parent_pin) || !pinned(store_pin)) return HELPER_NATIVE_UNCONFIRMED;
    return observe_mount(site,parent_pin,store_pin,want,false);
}
