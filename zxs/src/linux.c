#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/types.h>
#include <sys/xattr.h>
#include <unistd.h>

/* Input is one component relative to a live descriptor. No symlink lookup. */
int zxs_open_directory(int parent, const char *name) {
    return openat(parent, name, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
}

/* Does not consume the caller's directory cursor. Return 1 iff truly empty. */
int zxs_directory_empty(int fd) {
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
        if (strcmp(entry->d_name, ".") && strcmp(entry->d_name, "..")) { empty = 0; break; }
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
