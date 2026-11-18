#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/types.h>
#include <sys/xattr.h>
#include <unistd.h>

static int proc_fd_directory(void);

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

/* No unlabelled fallback on an unsupported or unreadable mount. */
int zxs_volume_label_present(int fd) {
    ssize_t bytes = fgetxattr(fd, "user.zxs.volume", NULL, 0);
    if (bytes >= 0) return 1;       /* An empty or unknown label is still a claim. */
    if (errno == ENODATA) return 0;
    return -1;
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
int zxs_anonymous_file(int fd) {
    return openat(fd, ".", O_TMPFILE | O_RDWR | O_CLOEXEC, 0600);
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
#include <linux/magic.h>
#include <sys/stat.h>
#include <sys/vfs.h>

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
#include <sys/ioctl.h>

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
int zxs_remove_name(int fd,const char *name) { return unlinkat(fd,name,0); }

/* Return length-prefixed opaque names, without opening any entry. Refuse the
 * directory rather than supplying an incomplete name scope on budget failure. */
ssize_t zxs_root_names(int fd,unsigned char *out,size_t cap) {
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
        if (!n || n>255 || ++count>8192 || 1+n>cap-used) { error=EOVERFLOW; break; }
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
