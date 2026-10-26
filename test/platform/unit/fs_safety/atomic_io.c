/* Isolated test-child interposer: covers libc write/fsync used by std::fs::File,
 * not rustix direct syscalls, all-process syscall counts, or physical power loss. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int kind(int fd) {
    const char *root = getenv("ZXS_ATOMIC_ROOT");
    if (!root) return 0;
    char marker[4096], descriptor[64], path[4096];
    snprintf(marker, sizeof(marker), "%s/.inject", root);
    if (access(marker, F_OK)) return 0;
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size < 0) return 0;
    path[size] = '\0';
    struct stat metadata;
    if (fstat(fd, &metadata)) return 0;
    if (S_ISDIR(metadata.st_mode) && !strcmp(path, root)) return 2;
    size_t length = strlen(root);
    return S_ISREG(metadata.st_mode) && !strncmp(path, root, length)
        && path[length] == '/' &&
        (!strncmp(path + length + 1, ".state-atomic-", 14)
         || !strcmp(path + length + 1, "source"));
}

static void record(const char *event) {
    FILE *log = fopen(getenv("ZXS_ATOMIC_LOG"), "a");
    if (!log) abort();
    if (fprintf(log, "%s\n", event) < 0 || fclose(log)) abort();
}

int fsync(int fd) {
    int target = kind(fd);
    const char *stage = getenv("ZXS_ATOMIC_STAGE");
    if (target) {
        const char *root = getenv("ZXS_ATOMIC_ROOT");
        char destination[4096];
        snprintf(destination, sizeof(destination), "%s/destination", root);
        struct stat source, published;
        if (fstat(fd, &source)) abort();
        if (target == 1) {
            /* A file barrier must precede publication of this exact inode. */
            if (!stat(destination, &published) && source.st_ino == published.st_ino
                && source.st_dev == published.st_dev) abort();
            record("file-sync");
        } else {
            if (stat(destination, &published) || !S_ISREG(published.st_mode)) abort();
            record("directory-sync");
        }
        if ((target == 1 && !strcmp(stage, "file-sync")) ||
            (target == 2 && !strcmp(stage, "directory-sync"))) {
            errno = EIO;
            return -1;
        }
        if (target == 1 && !strcmp(stage, "publish")) {
            /* Force a real publication conflict without intercepting rustix syscalls.
             * replace rejects the directory during its pre-rename type check. */
            if (!strcmp(getenv("ZXS_ATOMIC_MODE"), "replace")) {
                if (unlink(destination) || mkdir(destination, 0700)) abort();
            } else {
                int collision = open(destination, O_WRONLY | O_CREAT | O_EXCL, 0600);
                if (collision < 0 || close(collision)) abort();
            }
        }
    }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}

ssize_t write(int fd, const void *bytes, size_t count) {
    if (kind(fd) == 1 && !strcmp(getenv("ZXS_ATOMIC_STAGE"), "write")) {
        record("write-failure");
        errno = EIO;
        return -1;
    }
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    return real_write(fd, bytes, count);
}
