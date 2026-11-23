#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/magic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
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

int zxs_pin_record(int fd, const char *name) {
    return openat(fd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC);
}
/* No brand-specific feature: create a new ordinary private candidate. Never reopen or truncate a preexisting name. */
int zxs_create_new_regular(int parent, const char *name) {
    return openat(parent, name, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
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
