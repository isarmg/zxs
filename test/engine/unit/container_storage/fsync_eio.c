/* Test-only libc interposer: never linked into a production target. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int fsync(int fd) {
    char descriptor[64];
    char path[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size >= 0) {
        path[size] = '\0';
        struct stat metadata;
        if (fstat(fd, &metadata) == 0 && S_ISREG(metadata.st_mode)
            && strstr(path, "/file-containers/") && strstr(path, "/versions/v")) {
            fprintf(stderr, "ZXS_TEST_FSYNC_EIO %s\n", path);
            errno = EIO;
            return -1;
        }
    }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}
