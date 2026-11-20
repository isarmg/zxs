/* SPDX-License-Identifier: Apache-2.0 */
/* Specialize only the nonsecret SECRETS/SID key from the pinned original
 * standalone instance. No writable Samba getter/initialization, traversal,
 * password user keys, dynamic domain or alternate .tdb are exposed.
 */
static bool machine_named(int site, const struct statx *d, const struct statx *m) {
    int p = openat(site, "private", O_PATH | O_NOFOLLOW | O_CLOEXEC | O_DIRECTORY);
    if (p < 0) return false;
    int f = openat(p, "secrets.tdb", O_PATH | O_NOFOLLOW | O_CLOEXEC);
    bool matched = f >= 0 && quiet(p, d, true, false) && quiet(f, m, false, false);
    if (f >= 0) close(f);
    close(p);
    return matched;
}
// Samba secrets_store uses string_tdb_data, deliberately excluding the NUL.
static const char machine_sid_key[] = "SECRETS/SID/ZXSLOCAL";
static TDB_DATA machine_key(void) {
    return (TDB_DATA){(uint8_t *)machine_sid_key,sizeof(machine_sid_key)-1};
}
static int machine_sid(TDB_DATA key, TDB_DATA data, void *context) {
    (void)key;
    const uint8_t *sid = context;
    /* Debian amd64 4.22.11: struct dom_sid, zero-init + upstream sid_copy.
     * First 24 B is revision, authority and the four declared LE subauths;
     * all eleven unused subauth slots are zero. Never inspect another key. */
    const uint8_t zero[44] = {0};
    return data.dsize == 68 && memcmp(data.dptr, sid, 24) == 0
        && memcmp(data.dptr + 24, zero, sizeof(zero)) == 0 ? 0 : -1;
}
bool helper_native_machine_domain(int site, int private_dir, int machine,
    uint32_t uid, uint32_t gid, const uint8_t sid[24]) {
    const unsigned f = O_PATH;
    struct statx dir, db;
    char alias[64];
    int a=fcntl(site,F_GETFL), b=fcntl(private_dir,F_GETFL), c=fcntl(machine,F_GETFL);
    if (site < 3 || private_dir < 3 || machine < 3 || uid == 0 || gid == 0 || sid == NULL
        || a<0 || b<0 || c<0 || !(a & f) || !(b & f) || !(c & f)
        || !owned_stamp(private_dir,&dir,true,false,uid,gid)
        || !owned_stamp(machine,&db,false,false,uid,gid) || !same_mount(&dir,&db)
        || dir.stx_ino==db.stx_ino || !machine_named(site,&dir,&db)
        || !original_proc_path(machine,&db,false,false,alias)) return false;
    struct tdb_context *safe = tdb_open_ex(alias,0,TDB_NOMMAP,O_RDONLY | O_CLOEXEC | O_NOATIME | O_NONBLOCK,
        0,NULL,NULL);
    if (safe == NULL) return false;
    int opened = tdb_fd(safe), mode = fcntl(opened,F_GETFL);
    bool result = mode >= 0 && !(mode & O_PATH) && (mode & O_ACCMODE) == O_RDONLY && (mode & O_NOATIME)
        && quiet(opened,&db,false,false) && quiet(private_dir,&dir,true,false) && machine_named(site,&dir,&db)
        && tdb_parse_record(safe,machine_key(),machine_sid,(void *)sid) == 0
        && quiet(opened,&db,false,false);
    return tdb_close(safe) == 0 && result && quiet(machine,&db,false,false)
        && quiet(private_dir,&dir,true,false) && machine_named(site,&dir,&db);
}
