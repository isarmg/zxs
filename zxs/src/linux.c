#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/types.h>
#include <sys/mman.h>
#include <sys/vfs.h>
#include <linux/magic.h>
#include <sys/xattr.h>
#include <time.h>
#include <unistd.h>

static int proc_fd_directory(void);

/* Read descriptor evidence without changing it or choosing its owner. */
int observe_descriptor_flags(int fd) { return fcntl(fd, F_GETFD); }
int observe_file_seals(int fd) { return fcntl(fd, F_GET_SEALS); }

enum {
    STATUS_READ = 1, STATUS_WRITE = 2, STATUS_READ_WRITE = 3,
    STATUS_NONBLOCK = 4, STATUS_APPEND = 8
};
int observe_open_status(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) return flags;
    int access = 0;
    if (!(flags & O_PATH)) {
        switch (flags & O_ACCMODE) {
        case O_RDONLY: access = STATUS_READ; break;
        case O_WRONLY: access = STATUS_WRITE; break;
        case O_RDWR: access = STATUS_READ_WRITE; break;
        }
    }
    return access | ((flags & O_NONBLOCK) ? STATUS_NONBLOCK : 0) |
                    ((flags & O_APPEND) ? STATUS_APPEND : 0);
}

/* Input is one component relative to a live descriptor. No symlink lookup. */
int zxs_open_directory(int parent, const char *name) {
    return openat(parent, name, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
}

/* Per-directory native component limit. POSIX -1 with errno unchanged
   means indeterminate, not a promise that a 255-byte name is possible. */
int zxs_component_limit(int fd) {
    long n;
    do {
        errno = 0;
        n = fpathconf(fd, _PC_NAME_MAX);
    } while (n == -1 && errno == EINTR);
    if (n <= 0) {
        if (errno == 0) errno = EOPNOTSUPP;
        return -1;
    }
    return n > 255 ? 255 : (int)n;
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

/* An empty value is present; only ENODATA proves absence. */
int observe_xattr_presence(int fd, const char *name) {
    ssize_t bytes = fgetxattr(fd, name, NULL, 0);
    if (bytes >= 0) return 1;
    if (errno == ENODATA) return 0;
    return -1;
}

/* No unlabelled fallback on an unsupported or unreadable mount. */
int zxs_volume_label_present(int fd) {
    return observe_xattr_presence(fd, "user.zxs.volume");
}
uid_t zxs_effective_uid(void) { return geteuid(); }

#include <stdio.h>
#include <sys/random.h>

int zxs_random(unsigned char *buffer, size_t length) {
    while (length > 0) {
        ssize_t n = getrandom(buffer, length, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) return -1;
        if (n == 0) { errno = EIO; return -1; }
        buffer += n; length -= (size_t)n;
    }
    return 0;
}
int zxs_pin_record(int fd, const char *name) {
    return openat(fd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC);
}
/* No brand-specific feature: create a new ordinary private candidate. Never reopen or truncate a preexisting name. */
int zxs_create_new_regular(int parent, const char *name) {
    return openat(parent, name, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
}
int zxs_anonymous_file(int fd) {
    return openat(fd, ".", O_TMPFILE | O_RDWR | O_CLOEXEC, 0600);
}
/* NB: Rust positively caps this memory fallback; never turn permission or
 * ordinary disk-full into an unbounded alternate destination. */
int zxs_smb_scratch_fd(int parent, unsigned char *kind) {
    struct statfs fs;
    *kind=0;
    int fd=zxs_anonymous_file(parent);
    if (fd < 0) {
        if (errno != EOPNOTSUPP && errno != ENOSYS && errno != EISDIR && errno != EINVAL) return -1;
        fd=memfd_create("zxs-smb-stage", MFD_CLOEXEC);
        if (fd < 0) return -1;
        *kind=2; /* only the fallback is independent of the chosen F mount */
    }
    if (fstatfs(fd,&fs) != 0) { int saved=errno; close(fd); errno=saved; return -1; }
    if ((unsigned long)fs.f_type==TMPFS_MAGIC || (unsigned long)fs.f_type==RAMFS_MAGIC || *kind==2) *kind|=1;
    return fd;
}
/* linkat is inherently no-clobber. The proc descriptor refers only to the
 * still-owned anonymous inode, not to the registration pathname. */
int zxs_link_anonymous(int file, int dir, const char *name) {
    char path[32];
    int n = snprintf(path, sizeof path, "%d", file);
    if (n < 0 || (size_t)n >= sizeof path) { errno = EOVERFLOW; return -1; }
    int fds=proc_fd_directory();
    if (fds<0) return -1;
    int result=linkat(fds, path, dir, name, AT_SYMLINK_FOLLOW);
    int saved=errno; close(fds); errno=saved; return result;
}
int zxs_rename_fresh(int from, const char *old, int to, const char *next) {
    return renameat2(from, old, to, next, RENAME_NOREPLACE);
}
ssize_t zxs_read_label(int fd, unsigned char *buf, size_t n) {
    ssize_t read = fgetxattr(fd, "user.zxs.volume", buf, n);
    if (read < 0 && errno == ENODATA) return -2;
    return read;
}
int zxs_claim(int fd, const unsigned char *buf, size_t n) {
    return fsetxattr(fd, "user.zxs.volume", buf, n, XATTR_CREATE);
}

#include <stdint.h>
#include <sys/stat.h>
#include <sys/ioctl.h>
#include <sys/sysmacros.h>
#include <sys/syscall.h>
#include <linux/openat2.h>
#include <linux/capability.h>
#include <signal.h>

_Static_assert(sizeof(uid_t) == sizeof(uint32_t) && (uid_t)-1 > 0, "unsigned Linux uid");
_Static_assert(sizeof(gid_t) == sizeof(uint32_t) && (gid_t)-1 > 0, "unsigned Linux gid");

int observe_own_user_triplet(uint32_t (*out)[3]) {
    uid_t ids[3] = {0};
    int rc = getresuid(&ids[0], &ids[1], &ids[2]);
    if (rc == 0) for (size_t i = 0; i < 3; ++i) (*out)[i] = ids[i];
    return rc;
}
int observe_own_group_triplet(uint32_t (*out)[3]) {
    gid_t ids[3] = {0};
    int rc = getresgid(&ids[0], &ids[1], &ids[2]);
    if (rc == 0) for (size_t i = 0; i < 3; ++i) (*out)[i] = ids[i];
    return rc;
}
int observe_own_group_count(void) {
    return getgroups(0, NULL);
}

long observe_own_capability_words(uint32_t (*out)[2][3]) {
    _Static_assert(_LINUX_CAPABILITY_U32S_3 == 2, "original Linux capget version 3 word count");
    struct __user_cap_header_struct header = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct data[_LINUX_CAPABILITY_U32S_3] = {0};
    long rc = syscall(SYS_capget, &header, data);
    if (rc == 0) for (size_t i = 0; i < _LINUX_CAPABILITY_U32S_3; ++i) {
        (*out)[i][0] = data[i].effective;
        (*out)[i][1] = data[i].permitted;
        (*out)[i][2] = data[i].inheritable;
    }
    return rc;
}

int install_lowered_term(void (*handler)(int)) {
    struct sigaction action = {0};
    sigset_t term = {0};
    action.sa_handler = handler;
    /* No SA_RESTART: TERM must interrupt a blocked operation. */
    return sigemptyset(&action.sa_mask) == 0 &&
           sigemptyset(&term) == 0 &&
           sigaddset(&term, SIGTERM) == 0 &&
           sigaction(SIGTERM, &action, NULL) == 0 &&
           pthread_sigmask(SIG_UNBLOCK, &term, NULL) == 0;
}

#if defined(__x86_64__) && UINTPTR_MAX == UINT64_MAX
#include <linux/sched.h>
#include <linux/nsfs.h>
#include <sys/wait.h>

_Static_assert(sizeof(pid_t) == sizeof(int32_t) && (pid_t)-1 < 0, "signed Linux pid");

int32_t observe_own_task_ids(long *tid) {
    pid_t pid = getpid();
    *tid = syscall(SYS_gettid);
    return pid;
}

struct own_process_ids {
    int32_t pid;
    uint32_t uid, euid, gid, egid;
};
void observe_own_process_ids(struct own_process_ids *ids) {
    ids->pid = getpid();
    ids->uid = getuid();
    ids->euid = geteuid();
    ids->gid = getgid();
    ids->egid = getegid();
}

/* Auto-reaping would discard the original child exit evidence. */
int observe_waitable_children(void) {
    struct sigaction action = {0};
    return sigaction(SIGCHLD, NULL, &action) == 0 &&
           action.sa_handler == SIG_DFL && !(action.sa_flags & SA_NOCLDWAIT);
}

int install_foreground_stop(void (*handler)(int)) {
    struct sigaction action = {0};
    sigset_t mask = {0};
    action.sa_handler = handler;
    if (sigemptyset(&action.sa_mask) != 0 || sigprocmask(SIG_SETMASK, NULL, &mask) != 0)
        return 0;
    const int signals[] = {SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGTSTP};
    for (size_t i = 0; i < sizeof(signals) / sizeof(signals[0]); ++i) {
        int sig = signals[i];
        if (sigismember(&mask, sig) != 0 || sigaction(sig, &action, NULL) != 0)
            return 0;
    }
    return 1;
}

int reset_post_clone_signals(void) {
    struct sigaction action = {0};
    sigset_t empty = {0};
    action.sa_handler = SIG_DFL;
    if (sigemptyset(&action.sa_mask) != 0 || sigemptyset(&empty) != 0)
        return 0;
    for (int sig = 1; sig <= 64; ++sig) {
        /* KILL and STOP are immutable; 32 and 33 are libc-reserved. */
        if (sig == SIGKILL || sig == SIGSTOP || sig == 32 || sig == 33)
            continue;
        if (sigaction(sig, &action, NULL) != 0)
            return 0;
    }
    return sigprocmask(SIG_SETMASK, &empty, NULL) == 0;
}

struct namespace_probe { unsigned long request; int clone_type; };
/* Namespace tags in custody.rs order; Linux 6.12 PIDFS _IO request numbers. */
static const struct namespace_probe namespace_probes[] = {
    {_IO(0xff, 2), CLONE_NEWIPC},
    {_IO(0xff, 3), CLONE_NEWNS},
    {_IO(0xff, 4), CLONE_NEWNET},
    {_IO(0xff, 5), CLONE_NEWPID},
    {_IO(0xff, 6), CLONE_NEWPID},
    {_IO(0xff, 9), CLONE_NEWUSER},
    {_IO(0xff, 10), CLONE_NEWUTS},
};

int observe_namespace_kind(int fd, uint8_t kind) {
    return kind < sizeof(namespace_probes) / sizeof(namespace_probes[0]) &&
           ioctl(fd, NS_GET_NSTYPE, 0ul) == namespace_probes[kind].clone_type;
}
int open_process_namespace(int pidfd, uint8_t kind) {
    if (kind >= sizeof(namespace_probes) / sizeof(namespace_probes[0])) {
        errno = EINVAL;
        return -1;
    }
    return ioctl(pidfd, namespace_probes[kind].request, 0ul);
}
int open_namespace_owner(int nsfd) {
    return ioctl(nsfd, NS_GET_USERNS, 0ul);
}

/* No shared VM or vfork: the zero-result child returns on its copied stack. */
long clone_child_pidfd(int32_t *pidfd) {
    _Static_assert(sizeof(struct clone_args) == 88, "original clone3 ABI width");
    struct clone_args args = {
        .flags = CLONE_PIDFD | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS,
        .pidfd = (uintptr_t)pidfd,
        .exit_signal = SIGCHLD
    };
    return syscall(SYS_clone3, &args, sizeof(args));
}

struct child_exit_sample { int32_t pid, signal, code, status; };
static int read_child_exit(int pidfd, int flags, struct child_exit_sample *out) {
    siginfo_t info = {0};
    int rc = waitid(P_PIDFD, (id_t)pidfd, &info, flags);
    if (rc == 0) {
        *out = (struct child_exit_sample){info.si_pid, info.si_signo, info.si_code, info.si_status};
    }
    return rc;
}
int peek_child_exit(int pidfd, struct child_exit_sample *out) {
    return read_child_exit(pidfd, WEXITED | WNOHANG | WNOWAIT, out);
}
int reap_child_exit(int pidfd, struct child_exit_sample *out) {
    return read_child_exit(pidfd, WEXITED, out);
}
#endif

/* Linux v6.12 UAPI. Older libc headers may omit the unique mount ID bit. */
#ifndef STATX_MNT_ID_UNIQUE
#define STATX_MNT_ID_UNIQUE 0x00004000U
#endif

int observe_filesystem_type(int fd, long *out) {
    struct statfs observed = {0};
    if (fstatfs(fd, &observed) != 0) return -1;
    *out = (long)observed.f_type;
    return 0;
}

int observe_unique_mount(int fd, uint64_t *out) {
    struct statx observed = {0};
    if (statx(fd, "", AT_EMPTY_PATH | AT_NO_AUTOMOUNT, STATX_MNT_ID_UNIQUE, &observed) != 0 ||
        !(observed.stx_mask & STATX_MNT_ID_UNIQUE) || observed.stx_mnt_id == 0) return -1;
    *out = observed.stx_mnt_id;
    return 0;
}

int observe_mount_root(int fd) {
    struct statx observed = {0};
    if (statx(fd, "", AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_STATX_DONT_SYNC, STATX_MNT_ID, &observed) != 0 ||
        !(observed.stx_attributes_mask & STATX_ATTR_MOUNT_ROOT)) return -1;
    return (observed.stx_attributes & STATX_ATTR_MOUNT_ROOT) != 0;
}

struct character_device_identity {
    uint64_t device, inode, special_device;
    uint32_t major, minor;
};
int observe_character_device(int fd, struct character_device_identity *out) {
    struct stat observed = {0};
    if (fstat(fd, &observed) != 0 || !S_ISCHR(observed.st_mode) || observed.st_ino == 0) return -1;
    *out = (struct character_device_identity) {
        .device = (uint64_t)observed.st_dev, .inode = (uint64_t)observed.st_ino,
        .special_device = (uint64_t)observed.st_rdev, .major = major(observed.st_rdev), .minor = minor(observed.st_rdev)
    };
    return 0;
}

/* Each recipe keeps its original openat2 scope; all are read-only or O_PATH. */
enum {
    HELD_PROC_PIN = 1, HELD_PROC_DIRECTORY = 2, HELD_PROC_RECORD = 3,
    HELD_VIEW_PIN = 4, HELD_VIEW_LOCAL_PIN = 5,
    HELD_SITE_LOCAL_PIN = 6, HELD_SITE_CACHED_PIN = 7, HELD_SITE_DIRECTORY = 8
};
#define HELD_PIN_FLAGS (O_PATH | O_NOFOLLOW | O_CLOEXEC)
#define HELD_PROC_SCOPE (RESOLVE_BENEATH | RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS)
#define HELD_VIEW_SCOPE (RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)
#define HELD_SITE_SCOPE (HELD_VIEW_SCOPE | RESOLVE_NO_MAGICLINKS)
static const struct open_how held_read_recipes[] = {
    [HELD_PROC_PIN] = { .flags = HELD_PIN_FLAGS, .resolve = HELD_PROC_SCOPE },
    [HELD_PROC_DIRECTORY] = {
        .flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC, .resolve = HELD_PROC_SCOPE
    },
    [HELD_PROC_RECORD] = {
        .flags = O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC, .resolve = HELD_PROC_SCOPE
    },
    [HELD_VIEW_PIN] = { .flags = HELD_PIN_FLAGS, .resolve = HELD_VIEW_SCOPE },
    [HELD_VIEW_LOCAL_PIN] = { .flags = HELD_PIN_FLAGS, .resolve = HELD_VIEW_SCOPE | RESOLVE_NO_XDEV },
    [HELD_SITE_LOCAL_PIN] = { .flags = HELD_PIN_FLAGS, .resolve = HELD_SITE_SCOPE | RESOLVE_NO_XDEV },
    [HELD_SITE_CACHED_PIN] = { .flags = HELD_PIN_FLAGS, .resolve = HELD_SITE_SCOPE | RESOLVE_CACHED },
    [HELD_SITE_DIRECTORY] = {
        .flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC | O_NOATIME,
        .resolve = HELD_SITE_SCOPE | RESOLVE_NO_XDEV
    }
};
_Static_assert(sizeof(struct open_how) == 24, "original openat2 UAPI");
long open_held_read(int parent, const char *name, uint8_t recipe) {
    if (recipe == 0 || recipe >= sizeof(held_read_recipes) / sizeof(held_read_recipes[0])) {
        errno = EINVAL;
        return -1;
    }
    return syscall(SYS_openat2, parent, name, &held_read_recipes[recipe], sizeof(struct open_how));
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

#include <linux/fs.h>
#include <stdlib.h>

/* Queried on the actual live FD. The mount ID returned by name_to_handle_at
 * is intentionally not a durable identifier. We never open arbitrary handles. */
ssize_t zxs_native_birth(int fd, unsigned char *out, size_t cap) {
#ifdef FS_IOC_GETFSUUID
    enum { handle_capacity = 128, base = 16 + 4 };
    if (cap < base + handle_capacity) { errno = EINVAL; return -1; }
    struct fsuuid2 fs = {0};
    if (ioctl(fd, FS_IOC_GETFSUUID, &fs) < 0) return -1;
    if (fs.len != 16) { errno = ENOTSUP; return -1; }
    struct file_handle *h = calloc(1, sizeof(*h) + handle_capacity);
    if (!h) return -1;
    h->handle_bytes = handle_capacity;
    int mount_id;
    if (name_to_handle_at(fd, "", h, &mount_id, AT_EMPTY_PATH) < 0) {
        int e = errno; free(h); errno = e; return -1;
    }
    if (h->handle_bytes == 0 || h->handle_bytes > handle_capacity) {
        free(h); errno = ENOTSUP; return -1;
    }
    memcpy(out, fs.uuid, 16);
    uint32_t kind = (uint32_t)h->handle_type;
    for (unsigned int i=0; i<4; ++i) out[16+i]=(unsigned char)(kind>>(24-i*8));
    memcpy(out+base, h->f_handle, h->handle_bytes);
    ssize_t n=(ssize_t)(base+h->handle_bytes);
    free(h); return n;
#else
    (void)fd; (void)out; (void)cap; errno=ENOTSUP; return -1;
#endif
}
int zxs_open_pinned_record(int pin, int write) {
    struct stat st;
    if (fstat(pin,&st)<0) return -1;
    if (!S_ISREG(st.st_mode)) { errno=EINVAL; return -1; }
    char path[64];
    int n=snprintf(path,sizeof path,"%d",pin);
    if (n<0 || (size_t)n>=sizeof path) { errno=EOVERFLOW; return -1; }
    int fds=proc_fd_directory();
    if (fds<0) return -1;
    int result=openat(fds,path,(write ? O_RDWR : O_RDONLY)|O_NONBLOCK|O_NOCTTY|O_CLOEXEC);
    int e=errno; close(fds); errno=e; return result;
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

int zxs_put_material_token(int fd,const unsigned char *value,size_t n) {
    return fsetxattr(fd,"user.zxs.initial-file",value,n,XATTR_CREATE);
}
ssize_t zxs_get_material_token(int fd,unsigned char *value,size_t n) {
    return fgetxattr(fd,"user.zxs.initial-file",value,n);
}

/* The one original SYSTEM inode carries only this lower bound on confirmed
 * frames. Missing, short and oversized attributes are never treated as zero. */
ssize_t zxs_get_frontier(int fd,unsigned char *out,size_t len) {
    return fgetxattr(fd,"user.zxs.confirmed-frontier",out,len);
}
int zxs_set_frontier(int fd,const unsigned char *value,size_t len,int create) {
    return fsetxattr(fd,"user.zxs.confirmed-frontier",value,len,create?XATTR_CREATE:XATTR_REPLACE);
}

/* Only on the original transient SYSTEM allocation, never a resident
 * per-volume store. An absent binding is invalid, not a new empty one. */
ssize_t zxs_get_stage(int fd,unsigned char *out,size_t len) {
    return fgetxattr(fd,"user.zxs.create-stage",out,len);
}
int zxs_set_stage(int fd,const unsigned char *value,size_t len,int create) {
    return fsetxattr(fd,"user.zxs.create-stage",value,len,create?XATTR_CREATE:XATTR_REPLACE);
}

/* Reserve the one eventual physical extent file, not a second persistent
 * copy. Unsupported allocation, quota, capacity and overflow are failures. */
int zxs_allocate_stream(int fd,uint64_t total) {
    struct stat st;
    if (fstat(fd,&st)<0) return -1;
    off_t size=(off_t)total;
    if (!S_ISREG(st.st_mode)||st.st_nlink!=0||st.st_size!=0||size<=0||(uint64_t)size!=total) {errno=EINVAL;return -1;}
    return fallocate(fd,0,0,size); /* no sparse/truncate fallback */
}

/* Metadata only. Never open, then close, a SQLite DB inode for identity:
 * POSIX record locks on that inode would be lost by this entire process.
 * Only static, internally selected DB/control names are passed. */
int zxs_sql_private_name(int parent,const char *name,uint64_t *ino) {
    struct statx s;
    const unsigned int mask=STATX_TYPE|STATX_MODE|STATX_NLINK|STATX_UID|STATX_INO|STATX_MNT_ID;
    if (statx(parent,name,AT_SYMLINK_NOFOLLOW|AT_NO_AUTOMOUNT,mask,&s)<0) return -1;
    uint64_t mount=0;
    if (zxs_mount_id(parent,&mount)<0) return -1;
    if ((s.stx_mask&mask)!=mask || !S_ISREG(s.stx_mode) || (s.stx_mode&07777)!=0600 ||
        s.stx_uid!=geteuid() || s.stx_nlink!=1 || s.stx_ino==0 || s.stx_mnt_id!=mount) {
        errno=EINVAL; return -1;
    }
    *ino=s.stx_ino;
    return 0;
}

#include <sys/statvfs.h>
/* Translate native statvfs layout to the four ZXS mount flags. */
int zxs_mount_flags(int fd) {
    struct statvfs v;
    if (fstatvfs(fd,&v)!=0) return -1;
    unsigned long flags=v.f_flag;
    return ((flags&ST_RDONLY)!=0?1:0) | ((flags&ST_NOSUID)!=0?2:0) |
           ((flags&ST_NODEV)!=0?4:0) | ((flags&ST_NOEXEC)!=0?8:0);
}
/* Capacity of this currently mounted FS, not a per-folder quota or promise.
 * The full SMB sample keeps caller/free counts and the mount RO bit in
 * the same statvfs result. */
static int zxs_capacity_at(int fd,uint64_t *total,uint64_t *available,uint64_t *free_bytes,int *read_only) {
    struct statvfs v;
    if (fstatvfs(fd,&v)<0) return -1;
    uint64_t unit=(uint64_t)v.f_frsize,blocks=(uint64_t)v.f_blocks,avail=(uint64_t)v.f_bavail,free=(uint64_t)v.f_bfree;
    if (unit==0 || avail>blocks || blocks>UINT64_MAX/unit ||
        (free_bytes!=NULL && (avail>free || free>blocks))) { errno=EOVERFLOW; return -1; }
    *total=unit*blocks;*available=unit*avail;
    if(free_bytes!=NULL)*free_bytes=unit*free;
    if(read_only!=NULL)*read_only=(v.f_flag&ST_RDONLY)!=0;
    return 0;
}
int zxs_storage_capacity(int fd,uint64_t *total,uint64_t *available) {
    return zxs_capacity_at(fd,total,available,NULL,NULL);
}
int zxs_storage_capacity_full(int fd,uint64_t *total,uint64_t *available,uint64_t *free_bytes,int *read_only) {
    return zxs_capacity_at(fd,total,available,free_bytes,read_only);
}

#include <sys/socket.h>
#include <netinet/in.h>
#include <sys/un.h>
#include <stddef.h>

/* Linux v6.12 socket UAPI, for older host headers. */
#ifndef SO_PASSPIDFD
# if defined(__sparc__)
#  define SO_PASSPIDFD 0x0055
# elif defined(__hppa__)
#  define SO_PASSPIDFD 0x404a
# else
#  define SO_PASSPIDFD 76
# endif
#endif

enum {
    SOCKET_DOMAIN=1, SOCKET_TYPE=2, SOCKET_PROTOCOL=3, SOCKET_ACCEPTING=4,
    SOCKET_REUSE_PORT=5, SOCKET_PASS_CREDENTIALS=6, SOCKET_PASS_PIDFD=7
};
struct socket_integer_fact { int level, option; };
static const struct socket_integer_fact socket_integer_facts[] = {
    [SOCKET_DOMAIN] = {SOL_SOCKET, SO_DOMAIN},
    [SOCKET_TYPE] = {SOL_SOCKET, SO_TYPE},
    [SOCKET_PROTOCOL] = {SOL_SOCKET, SO_PROTOCOL},
    [SOCKET_ACCEPTING] = {SOL_SOCKET, SO_ACCEPTCONN},
    [SOCKET_REUSE_PORT] = {SOL_SOCKET, SO_REUSEPORT},
    [SOCKET_PASS_CREDENTIALS] = {SOL_SOCKET, SO_PASSCRED},
    [SOCKET_PASS_PIDFD] = {SOL_SOCKET, SO_PASSPIDFD},
};
static int socket_value(int fd, int level, int option, void *out, socklen_t width) {
    socklen_t bytes = width;
    if (getsockopt(fd, level, option, out, &bytes) != 0 || bytes != width) return -1;
    return 0;
}
int observe_socket_integer(int fd, uint8_t kind, int *out) {
    if (kind == 0 || kind >= sizeof(socket_integer_facts) / sizeof(socket_integer_facts[0])) return -1;
    struct socket_integer_fact fact = socket_integer_facts[kind];
    int value = 0;
    if (socket_value(fd, fact.level, fact.option, &value, sizeof(value)) != 0) return -1;
    *out = value;
    return 0;
}
int observe_socket_cookie(int fd, uint64_t *out) {
    uint64_t value = 0;
    if (socket_value(fd, SOL_SOCKET, SO_COOKIE, &value, sizeof(value)) != 0) return -1;
    *out = value;
    return 0;
}
struct socket_peer_credentials { int32_t pid; uint32_t uid, gid; };
int observe_socket_peer(int fd, struct socket_peer_credentials *out) {
    struct ucred value = {0};
    _Static_assert(sizeof(value) == 12, "Linux SO_PEERCRED width");
    if (socket_value(fd, SOL_SOCKET, SO_PEERCRED, &value, sizeof(value)) != 0) return -1;
    *out = (struct socket_peer_credentials){value.pid, value.uid, value.gid};
    return 0;
}

int observe_socket_unconnected(int fd) {
    struct sockaddr_storage address;
    memset(&address, 0, sizeof(address));
    socklen_t bytes = sizeof(address);
    return getpeername(fd, (struct sockaddr *)&address, &bytes) == -1 && errno == ENOTCONN ? 0 : -1;
}

#include <poll.h>

struct readiness_slot { int fd; uint8_t interest, observed; };
enum {
    WATCH_READ=1, WATCH_WRITE=2, WATCH_PEER_END=4, WATCH_ERROR=8,
    WATCH_HANG=16, WATCH_INVALID=32, WATCH_OTHER=64,
    MAX_READINESS_SLOTS=17 /* 16 listeners and their original supervisor. */
};
int wait_descriptor_readiness(struct readiness_slot *w,size_t count,int ms) {
    struct pollfd kernel[MAX_READINESS_SLOTS];
    if (count==0 || count>MAX_READINESS_SLOTS) { errno=EINVAL; return -1; }
    for (size_t i=0;i<count;i++) {
        unsigned int want=w[i].interest;
        kernel[i]=(struct pollfd){.fd=w[i].fd,
            .events=((want&WATCH_READ)?POLLIN:0) | ((want&WATCH_WRITE)?POLLOUT:0) |
                    ((want&WATCH_PEER_END)?POLLRDHUP:0), .revents=0};
    }
    int rc=poll(kernel,count,ms);
    for (size_t i=0;i<count;i++) {
        // Preserve partial results even on EINTR, including other native events.
        unsigned int seen=(unsigned short)kernel[i].revents;
        w[i].observed=((seen&POLLIN)?WATCH_READ:0) | ((seen&POLLOUT)?WATCH_WRITE:0) |
            ((seen&POLLRDHUP)?WATCH_PEER_END:0) | ((seen&POLLERR)?WATCH_ERROR:0) |
            ((seen&POLLHUP)?WATCH_HANG:0) | ((seen&POLLNVAL)?WATCH_INVALID:0) |
            ((seen&~(unsigned int)(POLLIN|POLLOUT|POLLRDHUP|POLLERR|POLLHUP|POLLNVAL))?WATCH_OTHER:0);
    }
    return rc;
}

int zxs_web_readable(int fd) {
    struct pollfd p = { .fd = fd, .events = POLLIN };
    int result = poll(&p,1,500);
    if (result == 0 || (result < 0 && errno == EINTR)) return 0;
    if (result < 0) return -1;
    if ((p.revents & POLLIN) != 0) return 1;
    errno = EIO; return -1;
}

int zxs_monotonic_sample(int64_t *seconds,int64_t *nanos) {
    struct timespec now;
    _Static_assert(sizeof(now.tv_sec)<=sizeof(*seconds) &&
                   sizeof(now.tv_nsec)<=sizeof(*nanos), "monotonic sample scalars fit int64_t");
    int rc=clock_gettime(CLOCK_MONOTONIC,&now);
    if (rc==0) {
        *seconds=now.tv_sec;
        *nanos=now.tv_nsec;
    }
    return rc;
}
