/* Child-only observation of activation_seals' first symlink_metadata boundary.
 * No production fault hook. The Rust scenario calibrates this observer against
 * checked_directory on the exact target pathname, including an absent target.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static void observe(const char *path) {
    const char *target = getenv("ZXS_ACTIVATION_PROBE_TARGET");
    const char *trigger = getenv("ZXS_ACTIVATION_PROBE_TRIGGER");
    const char *log = getenv("ZXS_ACTIVATION_PROBE_LOG");
    if (!path || !target || !trigger || !log || strcmp(path, target) != 0
        || access(trigger, F_OK) != 0) {
        return;
    }
    int fd = open(log, O_WRONLY | O_APPEND | O_CLOEXEC);
    if (fd < 0 || write(fd, "S", 1) != 1) {
        _exit(121);
    }
    if (close(fd) != 0) {
        _exit(122);
    }
}

#define OBSERVE_STAT(name, type) \
    int name(const char *path, type *status) { \
        int (*next)(const char *, type *) = dlsym(RTLD_NEXT, #name); \
        observe(path); \
        return next(path, status); \
    }

OBSERVE_STAT(stat, struct stat)
OBSERVE_STAT(lstat, struct stat)
OBSERVE_STAT(stat64, struct stat64)
OBSERVE_STAT(lstat64, struct stat64)

int statx(int directory, const char *path, int flags, unsigned int mask,
          struct statx *status) {
    int (*next)(int, const char *, int, unsigned int, struct statx *) =
        dlsym(RTLD_NEXT, "statx");
    observe(path);
    return next(directory, path, flags, mask, status);
}

int fstatat(int directory, const char *path, struct stat *status, int flags) {
    int (*next)(int, const char *, struct stat *, int) = dlsym(RTLD_NEXT, "fstatat");
    observe(path);
    return next(directory, path, status, flags);
}

int fstatat64(int directory, const char *path, struct stat64 *status, int flags) {
    int (*next)(int, const char *, struct stat64 *, int) = dlsym(RTLD_NEXT, "fstatat64");
    observe(path);
    return next(directory, path, status, flags);
}
