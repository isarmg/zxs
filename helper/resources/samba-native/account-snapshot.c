/* SPDX-License-Identifier: Apache-2.0 */
/* The fixed Debian Samba 4.22.11 tdbsam footprint, not a TDB interpreter.
 * Native libtdb checks and walks the original DB. The projection below only
 * validates the fixed SAMU v4 record's public fields and secret boundaries.
 * In particular no password-equivalent byte is compared, copied out or logged.
 */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include "account-snapshot.h"
#include <fcntl.h>
#include <linux/magic.h>
#include <stdio.h>
#include <sys/vfs.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <tdb.h>
#include <unistd.h>

#define ACCOUNT_FILE "passdb.tdb"
#define SAMU_LIMIT (1024U * 1024U)
#define STORE_LIMIT (64U * 1024U * 1024U)
#define USER_RID 1000U
#define ORDINARY_GROUP_RID 513U
#define ACB_NORMAL_ONLY 0x10U
#define SAMU_NEVER_EXPIRES UINT32_C(0x7fffffff)
#define SAMU_MIN_TIME UINT32_C(0x80000000)

struct field { const uint8_t *p; size_t n; };
static struct field tdb_field(TDB_DATA d) {
    return (struct field) { d.dptr, d.dsize };
}
static bool take(struct field *input, size_t n, struct field *out) {
    if (n > input->n) return false;
    if (out != NULL) *out = (struct field) { input->p, n };
    input->p += n;
    input->n -= n;
    return true;
}
static bool little(struct field *input, unsigned n, uint32_t *out) {
    struct field d;
    if ((n != 2 && n != 4) || !take(input, n, &d)) return false;
    *out = 0;
    for (unsigned i = 0; i < n; ++i) *out |= (uint32_t)d.p[i] << (8 * i);
    return true;
}
static bool counted(struct field *input, struct field *value) {
    uint32_t n;
    return little(input, 4, &n) && take(input, n, value);
}
static bool string_field(struct field v, const void *bytes, size_t n) {
    return v.n == n + 1 && v.p[n] == 0 && memcmp(v.p, bytes, n) == 0;
}
static bool empty_optional(struct field v) {
    return v.n == 0 || (v.n == 1 && v.p[0] == 0);
}
static bool basic(uint8_t c) {
    return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z');
}
static bool login_ok(const struct helper_native_expected *want) {
    size_t n = want->login_len;
    if (n == 0 || n > sizeof(want->login) || !basic(want->login[0])) return false;
    for (size_t i = 0; i < n; ++i) {
        uint8_t c = want->login[i];
        if (!basic(c) && c != '.' && c != '_' && c != '-') return false;
    }
    static const char *const denied[] = {"guest", "anonymous", "nobody", "root"};
    for (size_t i = 0; i < sizeof(denied) / sizeof(denied[0]); ++i) {
        if (n == strlen(denied[i]) && memcmp(want->login, denied[i], n) == 0) return false;
    }
    return true;
}
static bool epoch(const uint8_t raw[154], uint64_t *generation) {
    if (memcmp(raw, "ZXS1", 4) != 0 || raw[153] != 0) return false;
    size_t pos = 4;
    *generation = 0;
    for (unsigned i = 0; i < 5; ++i) {
        bool nonzero = false;
        if (raw[pos++] != ':') return false;
        for (unsigned k = 0; k < (i == 4 ? 16U : 32U); ++k) {
            uint8_t c = raw[pos++];
            unsigned digit;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = c - 'a' + 10;
            else return false;
            nonzero |= digit != 0;
            if (i == 4) *generation = (*generation << 4) | digit;
        }
        if (!nonzero) return false;
    }
    return pos == 153 && *generation <= INT64_MAX;
}
static bool expectations(const struct helper_native_expected *want) {
    uint64_t target, previous;
    if (want == NULL || !login_ok(want) || !epoch(want->target, &target)) return false;
    if (!want->has_previous) return target == 1;
    return epoch(want->previous, &previous) && target > 1 && previous == target - 1
        && memcmp(want->target, want->previous, 103) == 0
        && memcmp(want->target + 104, want->previous + 104, 32) != 0;
}

/* V4 uses V3 SAMU packing: unsigned fixed-size values and counted B fields.
 * Match the original desc bytes *before* a C-string getter discards suffix
 * bytes. Examine only the extents of NT/LM/history. Do not create a SAMU
 * through getters that consult or initialize the instance account policies.
 */
static enum helper_native_match samu(struct field b, const struct helper_native_expected *want) {
    struct field strings[12], hours;
    uint32_t rid, group, lm, nt, history, flags, divisions, advertised, ignored;
    uint32_t logoff, kickoff, last_set;
    if (b.n > SAMU_LIMIT || !little(&b, 4, &ignored)
        || !little(&b, 4, &logoff) || logoff != SAMU_NEVER_EXPIRES
        || !little(&b, 4, &kickoff) || kickoff != SAMU_NEVER_EXPIRES
        || !little(&b, 4, &ignored) || !little(&b, 4, &last_set)
        || last_set == 0 || last_set == SAMU_MIN_TIME || last_set == SAMU_NEVER_EXPIRES
        || !little(&b, 4, &ignored) || !little(&b, 4, &ignored))
        return HELPER_NATIVE_UNCONFIRMED;
    for (size_t i = 0; i < 12; ++i) {
        if (!counted(&b, &strings[i])) return HELPER_NATIVE_UNCONFIRMED;
    }
    if (!string_field(strings[0], want->login, want->login_len)
        || !string_field(strings[1], "ZXSLOCAL", 8)
        || !(empty_optional(strings[2]) || string_field(strings[2], want->login, want->login_len)))
        return HELPER_NATIVE_UNCONFIRMED;
    for (size_t i = 3; i < 12; ++i) {
        if (i != 8 && !empty_optional(strings[i])) return HELPER_NATIVE_UNCONFIRMED;
    }
    if (!little(&b, 4, &rid) || rid != USER_RID || !little(&b, 4, &group) || group != ORDINARY_GROUP_RID
        || !little(&b, 4, &lm) || lm != 0 || !little(&b, 4, &nt) || nt != 16 || !take(&b, nt, NULL)
        || !little(&b, 4, &history) || history != 0 || !little(&b, 4, &flags) || flags != ACB_NORMAL_ONLY
        || !little(&b, 2, &divisions) || divisions != 168 || !little(&b, 4, &advertised) || advertised != 21
        || !counted(&b, &hours) || hours.n != 32 || !little(&b, 2, &ignored)
        || !little(&b, 2, &ignored) || !little(&b, 4, &ignored) || b.n != 0)
        return HELPER_NATIVE_UNCONFIRMED;
    /* Only the visible 168 hours matter; V4 serializes a 32-byte array. */
    for (size_t i = 0; i < advertised; ++i) {
        if (hours.p[i] != 0xff) return HELPER_NATIVE_UNCONFIRMED;
    }
    if (strings[8].n != sizeof(want->target)) return HELPER_NATIVE_UNCONFIRMED;
    if (memcmp(strings[8].p, want->target, sizeof(want->target)) == 0) return HELPER_NATIVE_TARGET;
    if (want->has_previous && memcmp(strings[8].p, want->previous, sizeof(want->previous)) == 0)
        return HELPER_NATIVE_PREVIOUS;
    return HELPER_NATIVE_UNCONFIRMED;
}

struct traversal {
    const struct helper_native_expected *want;
    unsigned seen;
    bool failed;
    enum helper_native_match matched;
};
static bool record(TDB_DATA key, TDB_DATA data, struct traversal *s) {
    struct field k = tdb_field(key), v = tdb_field(data);
    unsigned bit = 0;
    uint32_t num;
    if (string_field(k, "INFO/version", 12)) {
        if (little(&v, 4, &num) && num == 4 && v.n == 0) bit = 1;
    } else if (string_field(k, "INFO/minor_version", 18)) {
        if (little(&v, 4, &num) && num == 0 && v.n == 0) bit = 2;
    } else if (string_field(k, "RID_000003e8", 12)) {
        if (string_field(v, s->want->login, s->want->login_len)) bit = 4;
    } else if (k.n > 5 && memcmp(k.p, "USER_", 5) == 0
               && string_field((struct field){k.p + 5, k.n - 5}, s->want->login, s->want->login_len)) {
        s->matched = samu(v, s->want);
        if (s->matched != HELPER_NATIVE_UNCONFIRMED) bit = 8;
    }
    if (bit == 0 || (s->seen & bit) != 0) {
        s->failed = true;
        return false;
    }
    s->seen |= bit;
    return true;
}
static int stored_key(TDB_DATA key, TDB_DATA data, void *context) {
    /* tdb_check calls this for physical live records, not obsolete data. */
    return record(key, data, context) ? 0 : -1;
}
static int live_key(struct tdb_context *db, TDB_DATA key, TDB_DATA data, void *context) {
    (void)db;
    /* A positive early-stop count from traverse is not success. */
    return record(key, data, context) ? 0 : 1;
}
static enum helper_native_match inspect(struct tdb_context *db, const struct helper_native_expected *want) {
    struct traversal physical = {.want = want}, linked = {.want = want};
    if (!expectations(want) || tdb_check(db, stored_key, &physical) != 0
        || physical.failed || physical.seen != 15
        || tdb_traverse_read(db, live_key, &linked) != 4 || linked.failed || linked.seen != 15
        || linked.matched != physical.matched)
        return HELPER_NATIVE_UNCONFIRMED;
    return linked.matched;
}

/* Compare this instant's full quiet original, never reuse it as a reboot ID. */
static bool stamp(int fd, struct statx *out, bool directory) {
    const unsigned mask = STATX_TYPE | STATX_MODE | STATX_UID | STATX_GID | STATX_NLINK |
                          STATX_INO | STATX_SIZE | STATX_MTIME | STATX_CTIME | STATX_MNT_ID;
    struct statvfs fs;
    return fstatvfs(fd, &fs) == 0 && (fs.f_flag & ST_RDONLY) != 0
        && statx(fd, "", AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT, mask, out) == 0
        && (out->stx_mask & mask) == mask && out->stx_mnt_id != 0 && out->stx_ino != 0
        && out->stx_uid == 0 && out->stx_gid == 0
        && (directory ? out->stx_mode == (S_IFDIR | 0700) && out->stx_nlink > 0
                       : out->stx_mode == (S_IFREG | 0600) && out->stx_nlink == 1
                         && out->stx_size > 0 && out->stx_size <= STORE_LIMIT);
}
static bool same_mount(const struct statx *a, const struct statx *b) {
    return a->stx_dev_major == b->stx_dev_major && a->stx_dev_minor == b->stx_dev_minor
        && a->stx_mnt_id == b->stx_mnt_id;
}
static bool quiet(int fd, const struct statx *old, bool directory) {
    struct statx now;
    return stamp(fd, &now, directory) && same_mount(old, &now) && old->stx_ino == now.stx_ino
        && old->stx_nlink == now.stx_nlink && old->stx_size == now.stx_size
        && old->stx_mtime.tv_sec == now.stx_mtime.tv_sec && old->stx_mtime.tv_nsec == now.stx_mtime.tv_nsec
        && old->stx_ctime.tv_sec == now.stx_ctime.tv_sec && old->stx_ctime.tv_nsec == now.stx_ctime.tv_nsec;
}
static bool names(const struct statx *directory, const struct statx *store) {
    const int flags = O_PATH | O_CLOEXEC | O_NOFOLLOW;
    int instance = open("/instance", flags | O_DIRECTORY);
    if (instance < 0) return false;
    int dir = openat(instance, "accountdb", flags | O_DIRECTORY);
    close(instance);
    if (dir < 0) return false;
    int pin = openat(dir, ACCOUNT_FILE, flags);
    bool result = pin >= 0 && quiet(dir, directory, true) && quiet(pin, store, false);
    if (pin >= 0) close(pin); /* O_PATH only, no competing lock-bearing open */
    close(dir);
    return result;
}
/* libtdb's opener takes a path, not an fd. Do not let it read even the
 * database header via a newly looked-up named file. Its sole direct target
 * is the already-held original through the actual private procfs table.
 * The one-shot worker runs in this same stable restricted namespace, with
 * procfs installed by its supervisor; it cannot replace mounts or change
 * the meaning of the fixed proc path.
 */
static bool proc_member(int fd, struct statx *where, unsigned type) {
    const unsigned mask = STATX_MNT_ID | STATX_TYPE;
    struct statfs info;
    return fstatfs(fd, &info) == 0 && info.f_type == PROC_SUPER_MAGIC
        && statx(fd, "", AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_SYMLINK_NOFOLLOW, mask, where) == 0
        && (where->stx_mask & mask) == mask && where->stx_mnt_id != 0
        && (where->stx_mode & S_IFMT) == type;
}
static bool original_proc_path(int original, const struct statx *expected, char path[64]) {
    const int pin = O_PATH | O_CLOEXEC | O_NOFOLLOW, dir = pin | O_DIRECTORY;
    char pid[16], number[16], actual[16];
    int n = snprintf(pid, sizeof(pid), "%ld", (long)getpid());
    int m = snprintf(number, sizeof(number), "%d", original);
    if (n <= 0 || n >= (int)sizeof(pid) || m <= 0 || m >= (int)sizeof(number)) return false;
    struct statx root, child;
    bool valid = false;
    int process = -1, table = -1, link = -1, target = -1;
    int proc = open("/proc", dir);
    if (proc < 0) return false;
    if (!proc_member(proc, &root, S_IFDIR)
        || !(root.stx_attributes_mask & STATX_ATTR_MOUNT_ROOT)
        || !(root.stx_attributes & STATX_ATTR_MOUNT_ROOT)
        || readlinkat(proc, "self", actual, sizeof(actual)) != n || memcmp(pid, actual, (size_t)n) != 0)
        goto done;
    process = openat(proc, pid, dir);
    if (process < 0 || !proc_member(process, &child, S_IFDIR) || !same_mount(&root, &child)) goto done;
    table = openat(process, "fd", dir);
    if (table < 0 || !proc_member(table, &child, S_IFDIR) || !same_mount(&root, &child)) goto done;
    link = openat(table, number, pin);
    if (link < 0 || !proc_member(link, &child, S_IFLNK) || !same_mount(&root, &child)) goto done;
    target = openat(table, number, O_PATH | O_CLOEXEC);
    if (target < 0 || !quiet(target, expected, false)) goto done;
    n = snprintf(path, 64, "/proc/%s/fd/%s", pid, number);
    valid = n > 0 && n < 64;
done:
    if (target >= 0) close(target);
    if (link >= 0) close(link);
    if (table >= 0) close(table);
    if (process >= 0) close(process);
    close(proc);
    return valid;
}
enum helper_native_match helper_native_observe(int parent_pin, int store_pin,
    const struct helper_native_expected *want) {
    struct statx p, f;
    const int directory_mode = fcntl(parent_pin, F_GETFL);
    const int file_mode = fcntl(store_pin, F_GETFL);
    if (!expectations(want) || directory_mode == -1 || file_mode == -1
        || !(directory_mode & O_PATH) || !(file_mode & O_PATH)
        || !stamp(parent_pin, &p, true) || !stamp(store_pin, &f, false)
        || !same_mount(&p, &f) || p.stx_ino == f.stx_ino || !names(&p, &f))
        return HELPER_NATIVE_UNCONFIRMED;

    /* Native O_RDONLY refuses recovery if writes are needed; no CREATE,
     * upgrade, storage-changing query, alternate path, or mode fallback.
     * libtdb itself disables byte locks on O_RDONLY; the cooperating offline
     * supervisor's true two EX leases are therefore a caller precondition.
     * The released build must omit TDB_TRACE; this worker has no log callback.
     */
    char alias[64];
    if (!original_proc_path(store_pin, &f, alias)) return HELPER_NATIVE_UNCONFIRMED;
    struct tdb_context *db = tdb_open_ex(alias, 0, TDB_NOMMAP,
        O_RDONLY | O_CLOEXEC | O_NOATIME | O_NONBLOCK, 0, NULL, NULL);
    if (db == NULL) return HELPER_NATIVE_UNCONFIRMED;
    int data_fd = tdb_fd(db);
    int mode = fcntl(data_fd, F_GETFL);
    enum helper_native_match result = HELPER_NATIVE_UNCONFIRMED;
    if (mode != -1 && !(mode & O_PATH) && (mode & O_ACCMODE) == O_RDONLY && (mode & O_NOATIME)
        && quiet(data_fd, &f, false) && quiet(parent_pin, &p, true) && names(&p, &f)) {
        result = inspect(db, want);
        if (!quiet(data_fd, &f, false)) result = HELPER_NATIVE_UNCONFIRMED;
    }
    if (tdb_close(db) != 0 || !quiet(store_pin, &f, false) || !quiet(parent_pin, &p, true)
        || !names(&p, &f)) return HELPER_NATIVE_UNCONFIRMED;
    return result;
}
