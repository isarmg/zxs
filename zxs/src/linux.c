#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/magic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/sysmacros.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/vfs.h>
#include <unistd.h>

/* Input is one component relative to a live descriptor. No symlink lookup. */
int zxs_open_directory(int parent, const char *name) {
    return openat(parent, name, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
}

/* Does not consume the caller's directory cursor. Return 1 iff truly empty. */
int zxs_directory_has_only(int fd, const char *allowed) {
    int held = zxs_open_directory(fd, ".");
    if (held < 0) return -1;
    DIR *directory = fdopendir(held);
    if (!directory) { int e = errno; close(held); errno = e; return -1; }
    int empty = 1;
    int e = 0;
    for (;;) {
        errno = 0;
        struct dirent *entry = readdir(directory);
        if (!entry) { e = errno; break; }
        if (strcmp(entry->d_name, ".") && strcmp(entry->d_name, "..") &&
            (!allowed || strcmp(entry->d_name, allowed))) { empty = 0; break; }
    }
    if (closedir(directory) < 0 && !e) e = errno;
    if (e) { errno = e; return -1; }
    return empty;
}

uid_t zxs_effective_uid(void) { return geteuid(); }

int zxs_import_take_inherited(int fd) {
    if (fd != 3 && fd != 4) { errno = EINVAL; return -1; }
    return fcntl(fd, F_SETFD, FD_CLOEXEC);
}
int zxs_import_socket(int fd, int parent) {
    int enabled = 0, type = 0, domain = 0;
    struct ucred peer;
    socklen_t n = sizeof(int);
    if (getsockopt(fd, SOL_SOCKET, SO_PASSCRED, &enabled, &n) < 0 ||
        n != sizeof(int) || enabled != 1) { errno = EACCES; return -1; }
    n = sizeof(int);
    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &n) < 0 ||
        n != sizeof(int) || type != SOCK_SEQPACKET) { errno = EACCES; return -1; }
    n = sizeof(int);
    if (getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &n) < 0 ||
        n != sizeof(int) || domain != AF_UNIX) { errno = EACCES; return -1; }
    n = sizeof(peer);
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &n) < 0 ||
        n != sizeof(peer) || peer.pid != parent || peer.uid != geteuid() || peer.gid != getegid()) {
        errno = EACCES; return -1;
    }
    return 0;
}
int zxs_import_parent(void) { return getppid(); }

int zxs_import_sealed(int fd) {
    const int required = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;
    struct stat stamp;
    if (fstat(fd, &stamp) < 0) return -1;
    if (!S_ISREG(stamp.st_mode) || stamp.st_uid != geteuid() || stamp.st_gid != getegid() || stamp.st_nlink != 0) {
        errno = EACCES;
        return -1;
    }
    int seals = fcntl(fd, F_GET_SEALS);
    if (seals < 0) return -1;
    if ((seals & required) != required) { errno = EACCES; return -1; }
    return 0;
}
int zxs_import_reader(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) return -1;
    if ((flags & (O_ACCMODE | O_PATH)) != O_RDONLY || !(flags & O_NOATIME)) {
        errno = EACCES; return -1;
    }
    return 0;
}
ssize_t zxs_import_send(int fd, const unsigned char *data, size_t size) {
    return send(fd, data, size, MSG_NOSIGNAL);
}
ssize_t zxs_import_receive(int fd, unsigned char *data, size_t size, int parent, int *body_fd) {
    *body_fd = -1;
    union {
        struct cmsghdr alignment;
        unsigned char bytes[CMSG_SPACE(sizeof(struct ucred)) + CMSG_SPACE(2 * sizeof(int))];
    } control;
    struct iovec row = { .iov_base = data, .iov_len = size };
    struct msghdr message = { .msg_iov = &row, .msg_iovlen = 1,
        .msg_control = control.bytes, .msg_controllen = sizeof control.bytes };
    ssize_t got = recvmsg(fd, &message, MSG_CMSG_CLOEXEC);
    if (got < 0) return got;
    int bad = got != (ssize_t)size || (message.msg_flags & (MSG_TRUNC | MSG_CTRUNC));
    int credentials = 0, rights = 0;
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&message); c; c = CMSG_NXTHDR(&message, c)) {
        if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS &&
            c->cmsg_len >= CMSG_LEN(0)) {
            size_t bytes = c->cmsg_len - CMSG_LEN(0);
            if (rights++ || bytes != sizeof(int)) bad = 1;
            for (size_t i = 0; i < bytes / sizeof(int); ++i) {
                int incoming;
                memcpy(&incoming, CMSG_DATA(c) + i * sizeof(int), sizeof incoming);
                if (*body_fd < 0) *body_fd = incoming;
                else { close(incoming); bad = 1; }
            }
        } else if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS &&
                   c->cmsg_len == CMSG_LEN(sizeof(struct ucred)) && !credentials) {
            struct ucred peer;
            memcpy(&peer, CMSG_DATA(c), sizeof peer);
            if (peer.pid != parent || peer.uid != geteuid() || peer.gid != getegid()) bad = 1;
            credentials = 1;
        } else bad = 1;
    }
    if (bad || !credentials) {
        if (*body_fd >= 0) close(*body_fd);
        *body_fd = -1;
        errno = EACCES;
        return -1;
    }
    return got;
}

int zxs_pin_record(int fd, const char *name) {
    return openat(fd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC);
}
/* No brand-specific feature: create a new ordinary private candidate. Never reopen or truncate a preexisting name. */
int zxs_create_new_regular(int parent, const char *name) {
    return openat(parent, name, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
}

/* A read-only capacity sample of the already held DATA staging directory. */
int zxs_data_available(int fd, uint64_t *out) {
    struct statvfs observed;
    if (fstatvfs(fd, &observed) < 0) return -1;
    if (observed.f_flag & ST_RDONLY) { errno = EROFS; return -1; }
    if (!observed.f_frsize || observed.f_bavail > observed.f_blocks ||
        observed.f_bavail > observed.f_bfree ||
        (uintmax_t)observed.f_bavail > UINT64_MAX / (uintmax_t)observed.f_frsize) {
        errno = EOVERFLOW;
        return -1;
    }
    *out = (uint64_t)observed.f_bavail * (uint64_t)observed.f_frsize;
    return 0;
}

int zxs_mount_id(int fd, uint64_t *out) {
    struct statx observed;
    if (statx(fd, "", AT_EMPTY_PATH | AT_NO_AUTOMOUNT, STATX_MNT_ID, &observed) < 0) return -1;
    if (!(observed.stx_mask & STATX_MNT_ID)) { errno = ENOTSUP; return -1; }
    *out = observed.stx_mnt_id;
    return 0;
}
static int require_same_mount(int parent, int child) {
    uint64_t a=0,b=0;
    if (zxs_mount_id(parent,&a)<0 || zxs_mount_id(child,&b)<0) return -1;
    if (a!=b) { errno=EXDEV; return -1; }
    return 0;
}
static int proc_self_directory(void) {
    int root=open("/proc", O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if (root<0) return -1;
    struct statfs filesystem;
    if (fstatfs(root,&filesystem)<0) { int e=errno; close(root); errno=e; return -1; }
    if (filesystem.f_type != PROC_SUPER_MAGIC) { close(root); errno=EINVAL; return -1; }
    /* self is the kernel's symlink to this process under this procfs root. */
    int self=openat(root,"self",O_RDONLY|O_DIRECTORY|O_CLOEXEC);
    if (self>=0 && require_same_mount(root,self)<0) { int e=errno; close(self); self=-1; errno=e; }
    int e=errno; close(root); errno=e; return self;
}
static int open_proc_child(const char *name,int flags) {
    int self=proc_self_directory();
    if (self<0) return -1;
    int result=openat(self,name,flags|O_CLOEXEC|O_NOFOLLOW);
    if (result>=0 && require_same_mount(self,result)<0) { int e=errno; close(result); result=-1; errno=e; }
    int e=errno; close(self); errno=e; return result;
}
static int proc_fd_directory(void) { return open_proc_child("fd",O_RDONLY|O_DIRECTORY); }
int zxs_open_mountinfo(void) { return open_proc_child("mountinfo",O_RDONLY); }

int zxs_open_lock_fdinfo(int target) {
    char name[32];
    if (target < 0) { errno = EBADF; return -1; }
    int n = snprintf(name, sizeof name, "%d", target);
    if (n < 0 || (size_t)n >= sizeof name) { errno = EOVERFLOW; return -1; }
    int parent = open_proc_child("fdinfo", O_RDONLY | O_DIRECTORY);
    if (parent < 0) return -1;
    int file = openat(parent, name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
    if (file >= 0 && require_same_mount(parent, file) < 0) {
        int e = errno;
        close(file);
        file = -1;
        errno = e;
    }
    int e = errno;
    close(parent);
    errno = e;
    return file;
}
uint64_t zxs_lock_device(unsigned int device_major, unsigned int device_minor) {
    return makedev(device_major, device_minor);
}

static int open_pinned_record(int pin, int flags) {
    struct stat st;
    if (fstat(pin,&st)<0) return -1;
    if (!S_ISREG(st.st_mode)) { errno=EINVAL; return -1; }
    char path[64];
    int n=snprintf(path,sizeof path,"%d",pin);
    if (n<0 || (size_t)n>=sizeof path) { errno=EOVERFLOW; return -1; }
    int fds=proc_fd_directory();
    if (fds<0) return -1;
    int result=openat(fds,path,flags|O_NONBLOCK|O_NOCTTY|O_CLOEXEC);
    int e=errno; close(fds); errno=e; return result;
}

int zxs_open_pinned_record(int pin, int write) {
    return open_pinned_record(pin, write ? O_RDWR : O_RDONLY);
}
int zxs_open_pinned_record_noatime(int pin) {
    return open_pinned_record(pin, O_RDONLY | O_NOATIME);
}

int zxs_open_noatime_directory(int parent) {
    int fd = openat(parent, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_NOATIME | O_CLOEXEC);
    if (fd >= 0 && require_same_mount(parent, fd) < 0) {
        int e = errno;
        close(fd);
        errno = e;
        return -1;
    }
    return fd;
}
ssize_t zxs_getdents_page(int fd, unsigned char *out, size_t size) {
    return syscall(SYS_getdents64, fd, out, size);
}

int zxs_mkdir_private(int fd,const char *name) { return mkdirat(fd,name,0700); }
int zxs_remove_empty_dir(int fd,const char *name) { return unlinkat(fd,name,AT_REMOVEDIR); }
int zxs_remove_name(int fd,const char *name) { return unlinkat(fd,name,0); }

/* Return length-prefixed opaque names, without opening any entry. Refuse the
 * directory rather than supplying an incomplete name scope on budget failure. */
ssize_t zxs_root_names(int fd,unsigned char *out,size_t cap,size_t max_entries) {
    int held=zxs_open_directory(fd,".");
    if (held<0) return -1;
    DIR *dir=fdopendir(held);
    if (!dir) { int e=errno; close(held); errno=e; return -1; }
    size_t used=0,count=0; int error=0;
    for (;;) {
        errno=0; struct dirent *entry=readdir(dir);
        if (!entry) { error=errno; break; }
        if (!strcmp(entry->d_name,".") || !strcmp(entry->d_name,"..")) continue;
        size_t n=strlen(entry->d_name);
        if (!n || n>255 || ++count>max_entries || 1+n>cap-used) { error=EOVERFLOW; break; }
        out[used++]=(unsigned char)n; memcpy(out+used,entry->d_name,n); used+=n;
    }
    if (closedir(dir)<0 && !error) error=errno;
    if (error) { errno=error; return -1; }
    return (ssize_t)used;
}
