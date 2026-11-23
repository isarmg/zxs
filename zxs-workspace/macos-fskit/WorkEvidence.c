#include "WorkEvidence.h"
#include <sys/attr.h>
#include <sys/mount.h>
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

int zxs_resident_work_leaf(int parent, const char *name, uint64_t inode, int64_t sec, int64_t nsec) {
    struct stat st;
    if (fstatat(parent, name, &st, AT_SYMLINK_NOFOLLOW) < 0) return errno;
    if (!S_ISREG(st.st_mode) || st.st_nlink != 1 || st.st_flags || st.st_uid != geteuid() ||
        (st.st_mode & ALLPERMS) != 0600 || st.st_ino != inode ||
        st.st_birthtimespec.tv_sec != sec || st.st_birthtimespec.tv_nsec != nsec) return ESTALE;
    return 0;
}

#include <dispatch/dispatch.h>
#include <pthread.h>
#include <sys/resource.h>

/* Open under Apple's per-thread no-materialization policy. Only this short-lived
 * native thread changes policy. No FSKit/shared worker inherits an altered policy. */
struct open_job {
    int parent, flags, fd, error;
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
        j->error = zxs_resident_work_leaf(j->parent, j->name, j->inode, j->sec, j->nsec);
        if (!j->error) {
            j->fd = openat(j->parent, j->name, j->flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
            if (j->fd < 0) j->error = errno;
        }
    }
    dispatch_semaphore_t done = j->done;
    dispatch_semaphore_signal(done);  /* This is the last access to the owner's stack. */
    dispatch_release(done);           /* Independent worker ref spans signal(). */
    return NULL; /* No other work ever runs on this thread. */
}

int zxs_open_work_leaf(int parent, const char *name, int flags, uint64_t inode, int64_t sec, int64_t nsec) {
    if (flags != O_RDONLY && flags != O_RDWR) { errno = EINVAL; return -1; }
    if (name == NULL || strnlen(name, 38) != 37 || strncmp(name, "body-", 5) ||
        strspn(name + 5, "0123456789abcdef") != 32) { errno = EINVAL; return -1; }
    struct open_job job = {.parent=parent, .flags=flags, .fd=-1, .name=name, .inode=inode, .sec=sec, .nsec=nsec};
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
