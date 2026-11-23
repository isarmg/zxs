#include "WorkEvidence.h"
#include <sys/attr.h>
#include <sys/mount.h>
#include <sys/param.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>
#if !defined(__APPLE__) || !defined(__LP64__)
#error "This private owner requires Darwin LP64"
#endif

/* Only compare a previously registered app-private APFS object. Never mint an
 * identity from a user-supplied directory/file, change flags/ACLs, or follow a link. */
int zxs_work_evidence(int fd, int directory, const char *volume_hex, uint64_t inode,
                      int64_t birth_sec, int64_t birth_nsec) {
    struct stat st;
    struct statfs fs;
    if (getuid() == 0 || getuid() != geteuid() || getgid() != getegid() ||
        volume_hex == NULL || strlen(volume_hex) != 32 || inode == 0 ||
        birth_sec <= 0 || birth_nsec < 0 || birth_nsec >= 1000000000) return EINVAL;
    if (fstat(fd, &st) < 0 || fstatfs(fd, &fs) < 0) return errno;
    if (st.st_uid != geteuid() || st.st_flags ||
        (directory ? (!S_ISDIR(st.st_mode) || (st.st_mode & ALLPERMS) != 0700) :
                     (!S_ISREG(st.st_mode) || st.st_nlink != 1 || (st.st_mode & ALLPERMS) != 0600)) ||
        st.st_ino != inode || st.st_birthtimespec.tv_sec != birth_sec ||
        st.st_birthtimespec.tv_nsec != birth_nsec || strcmp(fs.f_fstypename, "apfs") ||
        !(fs.f_flags & MNT_LOCAL) || (fs.f_flags & (MNT_RDONLY | MNT_IGNORE_OWNERSHIP | MNT_QUARANTINE)))
        return EPERM;
    filesec_t sec = filesec_init();
    if (!sec) return errno ? errno : EIO;
    int present = -1;
    int result = 0;
    if (fstatx_np(fd, &st, sec) < 0 || filesec_query_property(sec, FILESEC_ACL, &present) < 0)
        result = errno ? errno : EIO;
    else if (present != 0)
        result = EPERM;
    filesec_free(sec);
    if (result) return result;
    struct attrlist attrs = { .bitmapcount = ATTR_BIT_MAP_COUNT, .commonattr = ATTR_CMN_RETURNED_ATTRS,
                              .volattr = ATTR_VOL_INFO | ATTR_VOL_UUID };
    struct { uint32_t len; attribute_set_t ret; unsigned char uuid[16]; } out = {0};
    if (fgetattrlist(fd, &attrs, &out, sizeof(out), FSOPT_REPORT_FULLSIZE) < 0) return errno;
    if (out.len != sizeof(out) || out.ret.commonattr != ATTR_CMN_RETURNED_ATTRS ||
        (out.ret.volattr != ATTR_VOL_UUID && out.ret.volattr != (ATTR_VOL_INFO | ATTR_VOL_UUID)) ||
        out.ret.dirattr || out.ret.fileattr || out.ret.forkattr) return EIO;
    const char *digits = "0123456789abcdef";
    unsigned any = 0;
    for (size_t i = 0; i < 16; ++i) {
        unsigned c = out.uuid[i]; any |= c;
        if (volume_hex[2*i] != digits[c >> 4] || volume_hex[2*i+1] != digits[c & 15]) return ESTALE;
    }
    return any ? 0 : EIO;
}

/* A same-vnode lookup on insensitive APFS is not proof of its exact stored spelling. */
int zxs_work_spelling(int fd, const char *expected) {
    struct attrlist attrs = { .bitmapcount = ATTR_BIT_MAP_COUNT, .commonattr = ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME };
    struct { uint32_t len; attribute_set_t ret; attrreference_t ref; char bytes[256]; } out = {0};
    if (fgetattrlist(fd, &attrs, &out, sizeof(out), FSOPT_REPORT_FULLSIZE) < 0) return errno;
    size_t n = strlen(expected) + 1, at = (size_t)((char *)&out.ref - (char *)&out);
    if (out.len > sizeof(out) || out.len < at + sizeof(out.ref) || out.ret.commonattr != attrs.commonattr ||
        out.ret.volattr || out.ret.dirattr || out.ret.fileattr || out.ret.forkattr ||
        out.ref.attr_dataoffset < (int32_t)sizeof(out.ref) || (size_t)out.ref.attr_dataoffset > out.len - at ||
        n != out.ref.attr_length || n > out.len - at - (size_t)out.ref.attr_dataoffset) return ESTALE;
    return memcmp((char *)&out + at + out.ref.attr_dataoffset, expected, n) == 0 ? 0 : ESTALE;
}

static int resident_leaf(int parent, const char *name, mode_t mode, uint64_t inode, int64_t sec, int64_t nsec) {
    struct stat st;
    if (fstatat(parent, name, &st, AT_SYMLINK_NOFOLLOW) < 0) return errno;
    if ((S_ISDIR(mode) ? !S_ISDIR(st.st_mode) : (!S_ISREG(st.st_mode) || st.st_nlink != 1)) ||
        st.st_flags || st.st_uid != geteuid() ||
        (st.st_mode & ALLPERMS) != (mode & ALLPERMS) || st.st_ino != inode ||
        st.st_birthtimespec.tv_sec != sec || st.st_birthtimespec.tv_nsec != nsec) return ESTALE;
    return 0;
}

int zxs_resident_work_leaf(int parent, const char *name, uint64_t inode, int64_t sec, int64_t nsec) {
    return resident_leaf(parent,name,0600,inode,sec,nsec);
}

#include <dispatch/dispatch.h>
#include <pthread.h>
#include <sys/resource.h>

/* Open under Apple's per-thread no-materialization policy. Only this short-lived
 * native thread changes policy. No FSKit/shared worker inherits an altered policy. */
struct open_job {
    int parent, flags, fd, error;
    mode_t mode;
    const char *name;
    uint64_t inode;
    int64_t sec, nsec;
    dispatch_semaphore_t done;
};

static void *open_worker(void *p) {
    struct open_job *j = p;
    if (setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD,
                       IOPOL_MATERIALIZE_DATALESS_FILES_OFF) != 0 ||
        getiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_THREAD) !=
                       IOPOL_MATERIALIZE_DATALESS_FILES_OFF) {
        j->error = errno ? errno : EIO;
    } else {
        // The absolute Foundation App Group is only discovered here. An lstat on
        // the caller thread could materialize an ancestor before the protected open.
        struct stat before; int group=(j->mode==0);
        if (group) {
            if (lstat(j->name,&before)<0) j->error=errno;
            else if (!S_ISDIR(before.st_mode) || before.st_flags || before.st_uid!=getuid() ||
                     (before.st_mode&0022)) j->error=EPERM;
            else {
                j->mode=before.st_mode; j->inode=before.st_ino;
                j->sec=before.st_birthtimespec.tv_sec; j->nsec=before.st_birthtimespec.tv_nsec;
            }
        }
        if (!j->error) j->error=resident_leaf(j->parent,j->name,j->mode,j->inode,j->sec,j->nsec);
        if (!j->error) {
            j->fd = openat(j->parent, j->name, j->flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
            if (j->fd < 0) j->error = errno;
            else if (group) {
                struct stat held;
                if (fstat(j->fd,&held)<0) j->error=errno;
                else if (held.st_dev!=before.st_dev || held.st_ino!=before.st_ino || held.st_flags ||
                         held.st_uid!=getuid() || !S_ISDIR(held.st_mode) || (held.st_mode&0022)) j->error=ESTALE;
                else j->error=zxs_work_path(j->fd,j->name);
                if (j->error) { close(j->fd); j->fd=-1; }
            }
        }
    }
    dispatch_semaphore_t done = j->done;
    dispatch_semaphore_signal(done);  /* This is the last access to the owner's stack. */
    dispatch_release(done);           /* Independent worker ref spans signal(). */
    return NULL; /* No other work ever runs on this thread. */
}

static int open_resident(int parent, const char *name, int flags, mode_t mode,
                         uint64_t inode, int64_t sec, int64_t nsec) {
    struct open_job job = {.parent=parent,.flags=flags,.fd=-1,.name=name,.mode=mode,.inode=inode,.sec=sec,.nsec=nsec};
    job.done = dispatch_semaphore_create(0);
    if (!job.done) { errno = ENOMEM; return -1; }
    pthread_t thread;
    pthread_attr_t attr;
    int err = pthread_attr_init(&attr);
    if (!err) {
        err = pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
        if (!err) {
            dispatch_retain(job.done);
            err = pthread_create(&thread, &attr, open_worker, &job);
            if (err) dispatch_release(job.done);
        }
        pthread_attr_destroy(&attr);
    }
    if (!err) dispatch_semaphore_wait(job.done, DISPATCH_TIME_FOREVER);
    dispatch_release(job.done);
    if (err || job.error) { errno = err ? err : job.error; return -1; }
    return job.fd;
}

int zxs_open_work_leaf(int parent, const char *name, int flags, uint64_t inode, int64_t sec, int64_t nsec) {
    if ((flags != O_RDONLY && flags != O_RDWR) || name == NULL || strnlen(name,38)!=37 ||
        strncmp(name,"body-",5) || strspn(name+5,"0123456789abcdef")!=32) { errno=EINVAL; return -1; }
    return open_resident(parent,name,flags,0600,inode,sec,nsec);
}

/* Only FileManager's already-entitled, fixed App Group URL reaches this entry.
 * O_NOFOLLOW_ANY also forbids a symlink in an earlier component. Return an owned
 * directory FD for descriptor-relative work, never a public path capability. */
int zxs_work_path(int fd,const char *expected) {
    char path[MAXPATHLEN];
    if (!expected || expected[0]!='/') return EINVAL;
    if (fcntl(fd,F_GETPATH,path)<0) return errno;
    return strnlen(path,sizeof(path))<sizeof(path) && !strcmp(path,expected) ? 0 : ESTALE;
}
int zxs_open_group_dir(const char *path) {
    if (!path || path[0]!='/') { errno=EINVAL; return -1; }
    return open_resident(AT_FDCWD,path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW_ANY,0,0,0,0);
}

/* Registration candidates have a fixed grammar. A successful open alone NEVER
 * proves registration: compare the canonical immutable projection, host attempt,
 * APFS witnesses and raw pathname on the same held objects before use. */
int zxs_open_registration_leaf(int parent,const char *leaf,int directory,uint64_t ino,int64_t sec,int64_t nsec) {
    if (!leaf || (directory != 0 && directory != 1) ||
        (directory ? (strcmp(leaf,"names") && (strnlen(leaf,38)!=37 || strncmp(leaf,"work-",5) ||
                                              strspn(leaf+5,"0123456789abcdef")!=32))
                   : strcmp(leaf,"ticket"))) { errno=EINVAL; return -1; }
    return open_resident(parent,leaf,directory?(O_RDONLY|O_DIRECTORY):O_RDONLY,
                         directory?(S_IFDIR|0700):0600,ino,sec,nsec);
}

int zxs_open_marker(int parent, const char *name, uint64_t inode, int64_t sec, int64_t nsec) {
    static const char *const names[]={"host-attempt","provider-loaded","host-verified","host-stop","provider-stopped",
                                      "host-normal-gone","provider-local-clean","projection.json"};
    int match=0;
    if (name) for (size_t i=0; i<sizeof(names)/sizeof(names[0]); ++i) if (!strcmp(name,names[i])) match=1;
    if (!match) { errno=EINVAL; return -1; }
    int fd=open_resident(parent,name,O_RDONLY,0400,inode,sec,nsec);
    if (fd<0) return -1;
    filesec_t secinfo=filesec_init();
    int present=-1,e=0; struct stat st;
    if (!secinfo) e=errno?errno:EIO;
    else {
        if (fstatx_np(fd,&st,secinfo)<0 || filesec_query_property(secinfo,FILESEC_ACL,&present)<0) e=errno?errno:EIO;
        else if (present || !S_ISREG(st.st_mode) || (st.st_mode&ALLPERMS)!=0400 || st.st_flags || st.st_nlink!=1 ||
                 st.st_uid!=geteuid() || st.st_ino!=inode || st.st_birthtimespec.tv_sec!=sec ||
                 st.st_birthtimespec.tv_nsec!=nsec) e=ESTALE;
        filesec_free(secinfo);
    }
    if (e) { close(fd); errno=e; return -1; }
    return fd;
}

#include <dirent.h>
/* Immutable marker namespace only. The expected registered APFS identity/private
 * mode is independently checked on this same held FD by the caller. */
int zxs_new_names(int fd, int *sensitive) {
    struct attrlist attrs = { .bitmapcount=ATTR_BIT_MAP_COUNT, .commonattr=ATTR_CMN_RETURNED_ATTRS,
                              .volattr=ATTR_VOL_INFO | ATTR_VOL_CAPABILITIES };
    struct { uint32_t len; attribute_set_t ret; vol_capabilities_attr_t caps; } out = {0};
    if (fgetattrlist(fd,&attrs,&out,sizeof(out),FSOPT_REPORT_FULLSIZE)<0) return errno;
    uint32_t mask = VOL_CAP_FMT_CASE_SENSITIVE | VOL_CAP_FMT_CASE_PRESERVING;
    if (out.len!=sizeof(out) || out.ret.commonattr!=ATTR_CMN_RETURNED_ATTRS ||
        (out.ret.volattr!=ATTR_VOL_CAPABILITIES && out.ret.volattr!=attrs.volattr) ||
        out.ret.dirattr || out.ret.fileattr || out.ret.forkattr ||
        (out.caps.valid[VOL_CAPABILITIES_FORMAT]&mask)!=mask ||
        !(out.caps.capabilities[VOL_CAPABILITIES_FORMAT]&VOL_CAP_FMT_CASE_PRESERVING)) return ENOTSUP;
    int copy=openat(fd,".",O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if (copy<0) return errno;
    DIR *dir=fdopendir(copy);
    if (!dir) {int e=errno; close(copy); return e;}
    int e=0;
    for (;;) {
        errno=0; struct dirent *d=readdir(dir);
        if (!d) { e=errno; break; }
        if (strcmp(d->d_name,".") && strcmp(d->d_name,"..")) { e=ENOTEMPTY; break; }
    }
    if (closedir(dir)<0 && !e) e=errno;
    if (!e) *sensitive=!!(out.caps.capabilities[VOL_CAPABILITIES_FORMAT]&VOL_CAP_FMT_CASE_SENSITIVE);
    return e;
}

int zxs_marker_name(int fd, unsigned char *bytes, size_t cap, size_t *length) {
    struct attrlist attrs={.bitmapcount=ATTR_BIT_MAP_COUNT,.commonattr=ATTR_CMN_NAME|ATTR_CMN_RETURNED_ATTRS};
    struct {uint32_t len; attribute_set_t ret; attrreference_t ref; char buf[256];} out={0};
    if (fgetattrlist(fd,&attrs,&out,sizeof(out),FSOPT_REPORT_FULLSIZE)<0) return errno;
    size_t ref=(size_t)((char *)&out.ref-(char *)&out);
    if (out.len>sizeof(out) || out.len<ref+sizeof(out.ref) || out.ret.commonattr!=attrs.commonattr ||
        out.ret.volattr || out.ret.dirattr || out.ret.fileattr || out.ret.forkattr ||
        out.ref.attr_dataoffset<(int32_t)sizeof(out.ref) || (size_t)out.ref.attr_dataoffset>out.len-ref ||
        out.ref.attr_length<2 || out.ref.attr_length>out.len-ref-(size_t)out.ref.attr_dataoffset ||
        out.ref.attr_length>cap) return EIO;
    const char *start=(char *)&out+ref+out.ref.attr_dataoffset;
    size_t n=out.ref.attr_length-1;
    if (start[n] || memchr(start,0,n) || memchr(start,'/',n)) return EIO;
    memcpy(bytes,start,n); *length=n; return 0;
}
