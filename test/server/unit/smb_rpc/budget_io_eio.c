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
    return S_ISREG(metadata.st_mode) && !strncmp(path, root, length)
        && path[length] == '/' &&
        (strstr(path + length, ".budget.json") || strstr(path + length, ".budget-grow-"));
}

int fsync(int fd) {
    if (targeted(fd, "file-sync") || targeted(fd, "directory-sync")) {
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
