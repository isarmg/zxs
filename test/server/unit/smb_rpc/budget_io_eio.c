/* Child-only libc interposer; never linked into a production target. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int targeted(int fd, const char *stage) {
    const char *root = getenv("ZXS_SMB_BUDGET_FAULT_ROOT");
    const char *selected = getenv("ZXS_SMB_BUDGET_FAULT_STAGE");
    if (!root || !selected || strcmp(selected, stage)) return 0;
    char marker[4096], descriptor[64], path[4096];
    snprintf(marker, sizeof(marker), "%s/.inject", root);
    if (access(marker, F_OK)) return 0;
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size < 0) return 0;
    path[size] = '\0';
    struct stat metadata;
    if (fstat(fd, &metadata)) return 0;
    if (!strcmp(stage, "directory-sync"))
        return S_ISDIR(metadata.st_mode) && !strcmp(path, root);
    size_t length = strlen(root);
    if (!S_ISREG(metadata.st_mode) || strncmp(path, root, length)
        || path[length] != '/') return 0;
    const char *name = path + length + 1;
    if (strlen(name) != 50 || strncmp(name, ".state-atomic-", 14)
        || strcmp(name + 46, ".tmp")) return 0;
    for (size_t index = 14; index < 46; ++index)
        if (!((name[index] >= '0' && name[index] <= '9')
              || (name[index] >= 'a' && name[index] <= 'f'))) return 0;
    return 1;
}

int fsync(int fd) {
    int directory = targeted(fd, "directory-sync");
    if (directory) {
        static int first_retirement_sync_passed;
        const char *barrier = getenv("ZXS_HTTP_RETIREMENT_BARRIER");
        if (barrier && !strcmp(barrier, "second") && !first_retirement_sync_passed) {
            first_retirement_sync_passed = 1;
            int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
            return real_fsync(fd);
        }
    }
    if (targeted(fd, "file-sync") || directory) {
        errno = EIO;
        return -1;
    }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}

ssize_t write(int fd, const void *bytes, size_t count) {
    if (targeted(fd, "write")) {
        errno = EIO;
        return -1;
    }
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    return real_write(fd, bytes, count);
}
