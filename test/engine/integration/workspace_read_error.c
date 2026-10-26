/* Test-only read interposer. Compile as a shared object and preload only the
 * isolated workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror test.
 * No production hooks, privilege changes, ptrace, or socket access. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

ssize_t read(int fd, void *buffer, size_t count) {
    char descriptor[64], path[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size >= 0) {
        path[size] = '\0';
        char *suffix = strrchr(path, '.');
        const char *requested = getenv("ZXS_TEST_WORKSPACE_ERRNO");
        if (suffix && size < (ssize_t)sizeof(path) - 7
            && strcmp(suffix, ".json") == 0 && requested
            && strstr(path, "/workspaces/")) {
            strcpy(suffix, ".read-error");
            if (access(path, F_OK) == 0) {
                int failure = atoi(requested);
                if (failure == EIO || failure == EACCES) {
                    errno = failure;
                    return -1;
                }
            }
        }
    }
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return real_read(fd, buffer, count);
}
