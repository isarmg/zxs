/* Host-only destructive qualification of known, disposable synthetic Finder files.
 * No main, mount path, automatic mount/unmount, privileged action, or default build.
 * A future separately authorized harness supplies its original checked FSKit volume fsid,
 * held item FD pair, and exact known contents. Never pass a user's selected document. */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>
#if !defined(__APPLE__) || !defined(__LP64__)
#error "actual macOS FSKit mounted test required"
#endif

static int same_volume(int fd, fsid_t fsid) {
    struct statfs s;
    if (fstatfs(fd, &s) != 0) return 0;
    const uint32_t required = MNT_NOEXEC | MNT_NODEV | MNT_NOSUID;
    return (s.f_flags_ext & MNT_EXT_FSKIT) && (s.f_flags & required) == required &&
           s.f_owner == geteuid() && memcmp(&s.f_fsid, &fsid, sizeof(fsid)) == 0;
}

static int exact_read(int fd, off_t offset, size_t request, const char *data, size_t n) {
    unsigned char buf[96];
    if (request > sizeof(buf) || n > request) return 0;
    memset(buf, 0xa5, sizeof(buf));
    if (pread(fd, buf, request, offset) != (ssize_t)n || memcmp(buf, data, n)) return 0;
    for (size_t i=n; i<sizeof(buf); ++i) if (buf[i] != 0xa5) return 0;
    return 1;
}

static int size_is(int fd, off_t length) {
    struct stat s;
    return fstat(fd, &s) == 0 && S_ISREG(s.st_mode) && s.st_size == length && s.st_uid == getuid();
}

/* Keep the SAME already-open reader throughout. A residual zero-filled tail is a failure. */
int zxs_qualify_open_eof(int reader, int writer, fsid_t original_fsid) {
    const char seed[] = "Workspace FSKit disposable EOF case";
    struct stat r, w;
    if (getuid() == 0 || getuid() != geteuid() || reader == writer ||
        !same_volume(reader, original_fsid) || !same_volume(writer, original_fsid) ||
        (fcntl(reader, F_GETFL) & O_ACCMODE) != O_RDONLY || (fcntl(writer, F_GETFL) & O_ACCMODE) != O_RDWR ||
        fstat(reader, &r) || fstat(writer, &w) || !r.st_ino || r.st_dev != w.st_dev || r.st_ino != w.st_ino ||
        !size_is(reader, sizeof(seed)-1) || !exact_read(reader, 0, sizeof(seed)+6, seed, sizeof(seed)-1))
        return EINVAL;
    if (ftruncate(writer, 5) || !size_is(reader,5) || !exact_read(reader,3,20,"ks",2) ||
        !exact_read(reader,5,20,"",0) || ftruncate(writer,0) || !size_is(reader,0) ||
        !exact_read(reader,0,20,"",0) || !exact_read(reader,4,20,"",0) ||
        pwrite(writer,"new",3,0) != 3 || !size_is(reader,3) || !exact_read(reader,0,20,"new",3))
        return EIO;
    return 0;
}

/* Future harness supplies two known originals at its private, matching target. Never accept
 * unsupported replace as a working editor save. Positively test refusal and support separately. */
int zxs_qualify_editor_replacement(int parent, int old_reader, fsid_t original_fsid, int expect_support) {
    const char *old = "eof-probe", *tmp = "replace-probe";
    const char src[] = "Workspace FSKit temp";
    int fd = -1, now = -1, result = EIO;
    struct stat before, named, other;
    if ((expect_support != 0 && expect_support != 1) || getuid()==0 || getuid()!=geteuid() ||
        !same_volume(parent,original_fsid) || !same_volume(old_reader,original_fsid) ||
        fstat(old_reader,&before) || !size_is(old_reader,3) || !exact_read(old_reader,0,12,"new",3) ||
        fstatat(parent,old,&named,AT_SYMLINK_NOFOLLOW) || named.st_dev!=before.st_dev ||
        named.st_ino!=before.st_ino || before.st_nlink!=1) return EINVAL;
    fd = openat(parent,tmp,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if (fd < 0 || !same_volume(fd,original_fsid) || fstat(fd,&other) || other.st_dev!=before.st_dev ||
        other.st_ino==before.st_ino || other.st_nlink!=1 || !size_is(fd,sizeof(src)-1) ||
        !exact_read(fd,0,sizeof(src)+3,src,sizeof(src)-1)) goto done;
    if (renameat(parent,tmp,parent,old) != 0) {
        int code = errno;
        if (!expect_support && (code==EROFS || code==ENOTSUP || code==ENOSYS || code==EPERM) &&
            fstatat(parent,old,&named,AT_SYMLINK_NOFOLLOW)==0 && named.st_ino==before.st_ino &&
            fstatat(parent,tmp,&named,AT_SYMLINK_NOFOLLOW)==0 && named.st_ino==other.st_ino &&
            exact_read(old_reader,0,12,"new",3)) result=0;
        goto done;
    }
    if (!expect_support) goto done;
    now = openat(parent,old,O_RDONLY|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if (now>=0 && same_volume(now,original_fsid) && fstat(now,&named)==0 && named.st_ino==other.st_ino &&
        exact_read(old_reader,0,12,"new",3) && exact_read(now,0,sizeof(src)+3,src,sizeof(src)-1)) result=0;
done:
    if (now>=0 && close(now)) result=EIO;
    if (fd>=0 && close(fd)) result=EIO;
    return result;
}
