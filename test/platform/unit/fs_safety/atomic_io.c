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

#ifndef ZXS_STAGING_PREFIX
#define ZXS_STAGING_PREFIX ".state-atomic-"
#endif

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
        (!strncmp(path + length + 1, ZXS_STAGING_PREFIX, strlen(ZXS_STAGING_PREFIX))
         || !strncmp(path + length + 1, ".zxs-payload-", 13)
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
        const char *entry = getenv("ZXS_ATOMIC_DESTINATION");
        snprintf(destination, sizeof(destination), "%s/%s", root, entry ? entry : "destination");
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
            const char *mode = getenv("ZXS_ATOMIC_MODE");
            if (!strcmp(mode, "replace") || !strcmp(mode, "replace-held") || !strcmp(mode, "staged-replace")) {
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

/* The libc-backend test build reaches these real rename calls. After-success
 * injection models a filesystem reporting an error after changing namespace. */
static int rename_target(int olddirfd, const char *oldpath, int newdirfd, const char *newpath) {
    const char *root = getenv("ZXS_ATOMIC_ROOT");
    const char *stage = getenv("ZXS_ATOMIC_STAGE");
    if (!root || !stage || (strcmp(stage, "rename-before") && strcmp(stage, "rename-after") && strcmp(stage, "create-race"))) return 0;
    char marker[4096], descriptor[64], path[4096];
    snprintf(marker, sizeof(marker), "%s/.inject", root);
    if (access(marker, F_OK) || (strncmp(oldpath, ZXS_STAGING_PREFIX, strlen(ZXS_STAGING_PREFIX))
        && strncmp(oldpath, ".zxs-payload-", 13) && strcmp(oldpath, "source"))) return 0;
    const char *destination = getenv("ZXS_ATOMIC_DESTINATION");
    if (strcmp(newpath, destination ? destination : "destination")) return 0;
    for (int i = 0; i < 2; i++) {
        snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", i ? newdirfd : olddirfd);
        ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
        if (size < 0) return 0;
        path[size] = '\0';
        if (strcmp(path, root)) return 0;
    }
    return 1;
}

static int rename_result(int result, int newdirfd, const char *newpath, const struct stat *before) {
    if (result) abort(); /* This regression must actually rename successfully. */
    struct stat after;
    if (fstatat(newdirfd, newpath, &after, AT_SYMLINK_NOFOLLOW)
        || after.st_ino != before->st_ino || after.st_dev != before->st_dev) abort();
    record("rename-success-reported-eio");
    errno = EIO;
    return -1;
}

int renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath) {
    int (*real_renameat)(int, const char *, int, const char *) = dlsym(RTLD_NEXT, "renameat");
    if (!rename_target(olddirfd, oldpath, newdirfd, newpath)) return real_renameat(olddirfd, oldpath, newdirfd, newpath);
    if (!strcmp(getenv("ZXS_ATOMIC_STAGE"), "rename-before")) {
        record("rename-before-eio"); errno = EIO; return -1;
    }
    struct stat before;
    if (fstatat(olddirfd, oldpath, &before, AT_SYMLINK_NOFOLLOW)) abort();
    return rename_result(real_renameat(olddirfd, oldpath, newdirfd, newpath), newdirfd, newpath, &before);
}

int renameat2(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, unsigned int flags) {
    int (*real_renameat2)(int, const char *, int, const char *, unsigned int) = dlsym(RTLD_NEXT, "renameat2");
    if (!rename_target(olddirfd, oldpath, newdirfd, newpath)) return real_renameat2(olddirfd, oldpath, newdirfd, newpath, flags);
    if (!strcmp(getenv("ZXS_ATOMIC_STAGE"), "rename-before")) {
        record("rename-before-eio"); errno = EIO; return -1;
    }
    if (!strcmp(getenv("ZXS_ATOMIC_STAGE"), "create-race")) {
        /* Occupy the name after the Rust precheck, then obtain a real EEXIST. */
        int occupant = openat(newdirfd, newpath, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
        if (occupant < 0 || write(occupant, "occupant", 8) != 8 || close(occupant)) abort();
        int result = real_renameat2(olddirfd, oldpath, newdirfd, newpath, flags);
        if (result != -1 || errno != EEXIST) abort();
        record("rename-race-eexist");
        errno = EEXIST;
        return result;
    }
    struct stat before;
    if (fstatat(olddirfd, oldpath, &before, AT_SYMLINK_NOFOLLOW)) abort();
    return rename_result(real_renameat2(olddirfd, oldpath, newdirfd, newpath, flags), newdirfd, newpath, &before);
}
