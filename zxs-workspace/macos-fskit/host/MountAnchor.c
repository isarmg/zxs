#include "MountAnchor.h"
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/attr.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>
#if !defined(__APPLE__) || !defined(__LP64__)
#error "Original native mount evidence requires the macOS LP64 SDK"
#endif

struct zxs_mount_anchor {
    struct statfs *before;
    int count, parent, root, used, unknown, detached;
    struct stat parent_stat, root_stat;
    struct statfs mount_stat;
    char target[MNAMELEN];
    unsigned char uuid[16];
    uint64_t item;
};

static int ordinary(void) {
    return getuid() && getuid() == geteuid() && getgid() == getegid();
}
static int samefs(fsid_t a, fsid_t b) { return !memcmp(&a, &b, sizeof(a)); }
static int samestat(struct stat a, struct stat b) { return a.st_dev==b.st_dev && a.st_ino==b.st_ino; }

static int snapshot(struct statfs **rows, int *count) {
    *rows = NULL;
    int n = getmntinfo_r_np(rows, MNT_NOWAIT);
    if (n <= 0 || !*rows) { int e=errno; free(*rows); *rows=NULL; return e ? e : EIO; }
    if (n > 16384) { free(*rows); *rows=NULL; return EOVERFLOW; }
    *count = n;
    return 0;
}
static int gate_parent(struct zxs_mount_anchor *a) {
    struct stat named, held;
    if (!ordinary()) return EPERM;
    if (lstat("/Volumes", &named) || fstat(a->parent, &held)) return errno;
    if (!samestat(named, held) || !samestat(held, a->parent_stat) || !S_ISDIR(held.st_mode) ||
        held.st_uid != 0 || (held.st_mode & 0022)) return ESTALE;
    return 0;
}
zxs_mount_anchor *zxs_mount_before(void) {
    if (!ordinary()) { errno=EPERM; return NULL; }
    struct zxs_mount_anchor *a=calloc(1,sizeof(*a));
    if (!a) return NULL;
    a->root=-1;
    a->parent=open("/Volumes", O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    int e=0;
    if (a->parent<0 || fstat(a->parent,&a->parent_stat)<0) e=errno;
    if (!e) e=gate_parent(a);
    if (!e) e=snapshot(&a->before,&a->count);
    if (e) { zxs_mount_release(a); errno=e; return NULL; }
    return a;
}

static int root_uuid(int fd, const unsigned char expected[16]) {
    struct attrlist attrs={.bitmapcount=ATTR_BIT_MAP_COUNT,.commonattr=ATTR_CMN_RETURNED_ATTRS,
                           .volattr=ATTR_VOL_INFO|ATTR_VOL_UUID};
    struct {uint32_t len; attribute_set_t ret; unsigned char uuid[16];} out={0};
    if (fgetattrlist(fd,&attrs,&out,sizeof(out),FSOPT_REPORT_FULLSIZE)<0) return errno;
    if (out.len!=sizeof(out) || out.ret.commonattr!=attrs.commonattr || out.ret.dirattr ||
        out.ret.fileattr || out.ret.forkattr ||
        (out.ret.volattr!=ATTR_VOL_UUID && out.ret.volattr!=attrs.volattr) || memcmp(out.uuid,expected,16))
        return ESTALE;
    return 0;
}
static int match_mount(struct zxs_mount_anchor *a, const struct statfs *s) {
    unsigned required=MNT_NOEXEC|MNT_NODEV|MNT_NOSUID;
    if (!(s->f_flags_ext & MNT_EXT_FSKIT) || s->f_owner!=getuid() || (s->f_flags & required)!=required ||
        (s->f_flags & (MNT_RDONLY|MNT_IGNORE_OWNERSHIP|MNT_UNION)) ||
        strnlen(s->f_fstypename,sizeof(s->f_fstypename))!=strlen("zxswork") ||
        memcmp(s->f_fstypename,"zxswork",strlen("zxswork")+1) ||
        strnlen(s->f_mntonname,sizeof(s->f_mntonname))>=sizeof(s->f_mntonname) ||
        strcmp(s->f_mntonname,a->target) || !samefs(s->f_fsid,a->mount_stat.f_fsid)) return ESTALE;
    return 0;
}

/* Match BOTH the issued ID and the only target. A reused ID, replacement mount
 * or inconsistent table is unknown, never permission to address that object. */
static int table(struct zxs_mount_anchor *a, int absent) {
    struct statfs *rows=NULL;
    int n=0, e=snapshot(&rows,&n), matches=0;
    if (e) return e;
    for (int i=0; i<n; ++i) {
        struct statfs *s=&rows[i];
        if (strnlen(s->f_mntonname,sizeof(s->f_mntonname))>=sizeof(s->f_mntonname)) { e=EIO; break; }
        if (!samefs(s->f_fsid,a->mount_stat.f_fsid) && strcmp(s->f_mntonname,a->target)) continue;
        if (absent || ++matches!=1 || (e=match_mount(a,s))) { if (!e) e=ESTALE; break; }
    }
    if (!e && !absent && matches!=1) e=ESTALE;
    free(rows);
    return e;
}

int zxs_mount_bind(zxs_mount_anchor *a, const char *path, const unsigned char uuid[16], uint64_t root) {
    if (!a || a->used || !path || !uuid || !root) return EINVAL;
    a->used=1; a->unknown=1; /* Failure after mount never authorizes a different URL or a cleanup by path. */
    size_t n=strnlen(path,sizeof(a->target));
    size_t prefix=sizeof("/Volumes/")-1;
    static const unsigned char zero[16]={0};
    if (n>=sizeof(a->target) || n<=prefix || memcmp(path,"/Volumes/",prefix) ||
        strchr(path+prefix,'/') || !strcmp(path+prefix,".") || !strcmp(path+prefix,"..") ||
        !memcmp(uuid,zero,16)) return EINVAL;
    int e=gate_parent(a);
    if (e) return e;
    memcpy(a->target,path,n+1); memcpy(a->uuid,uuid,16); a->item=root;
    a->root=openat(a->parent,path+prefix,O_EVTONLY|O_DIRECTORY|O_NONBLOCK|O_NOFOLLOW|O_CLOEXEC);
    if (a->root<0 || fstat(a->root,&a->root_stat) || fstatfs(a->root,&a->mount_stat)) return errno;
    fsid_t empty={0};
    if (samefs(a->mount_stat.f_fsid,empty)) return ESTALE;
    for (int i=0; i<a->count; ++i) {
        const struct statfs *old=&a->before[i];
        if (strnlen(old->f_mntonname,sizeof(old->f_mntonname))>=sizeof(old->f_mntonname) ||
            samefs(old->f_fsid,a->mount_stat.f_fsid) || !strcmp(old->f_mntonname,path)) return ESTALE;
    }
    a->unknown=0;
    e=zxs_mount_check(a);
    if (e) a->unknown=1;
    return e;
}

int zxs_mount_check(zxs_mount_anchor *a) {
    if (!a || a->unknown || a->detached || a->root<0) return ESTALE;
    int e=gate_parent(a);
    if (e) return e;
    struct stat named,held;
    struct statfs fs;
    if (fstatat(a->parent,a->target+sizeof("/Volumes/")-1,&named,AT_SYMLINK_NOFOLLOW) ||
        fstat(a->root,&held) || fstatfs(a->root,&fs)) return errno;
    if (!samestat(named,held) || !samestat(held,a->root_stat) || !S_ISDIR(held.st_mode) ||
        (held.st_mode & ALLPERMS)!=0500 || held.st_uid!=getuid() || held.st_gid!=getgid() ||
        held.st_ino!=a->item) return ESTALE;
    if ((e=match_mount(a,&fs)) || (e=root_uuid(a->root,a->uuid)) || (e=table(a,0))) return e;
    return 0;
}

int zxs_mount_normal(zxs_mount_anchor *a) {
    int e=zxs_mount_check(a);
    if (e) { if (a) a->unknown=1; return e; }
    /* Public Darwin funmount checks VROOT on this exact held FD. No force,
     * alternate target, command, installation or automatic retry exists here. */
    if (funmount(a->root,0)) {
        e=errno;
        if (e!=EBUSY || zxs_mount_check(a)) { a->unknown=1; return e==EBUSY ? ESTALE : e; }
        return EBUSY;
    }
    a->unknown=1;
    if ((e=gate_parent(a)) || (e=table(a,1))) return e;
    a->detached=1;
    return 0;
}

void zxs_mount_release(zxs_mount_anchor *a) {
    if (!a) return;
    if (a->root>=0) close(a->root);
    if (a->parent>=0) close(a->parent);
    free(a->before); free(a);
}
