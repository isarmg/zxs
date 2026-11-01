/* Only libc fsync in an isolated test child; no physical power-loss claim. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int fsync(int fd) {
    static int injected;
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    const char *root = getenv("ZXS_DIRECTORY_ROOT");
    if (!root) return real_fsync(fd);
    char link[64], path[4096], child[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(link, path, sizeof(path) - 1);
    if (size < 0) abort();
    path[size] = '\0';
    snprintf(child, sizeof(child), "%s/branch", root);
    /* Trusted creation synchronizes existing ancestors before mkdir. Arm only
     * once the new child exists, so the failure belongs to its creation. */
    if (access(child, F_OK)) return real_fsync(fd);
    int fail = !injected && !strcmp(path,
        !strcmp(getenv("ZXS_DIRECTORY_STAGE"), "child") ? child : root);
    int result = fail ? -1 : real_fsync(fd);
    int saved_errno = fail ? EIO : errno;
    FILE *log = fopen(getenv("ZXS_DIRECTORY_LOG"), "a");
    if (!log) abort();
    if (fprintf(log, "%s %s\n", result ? "fail" : "sync", path) < 0 || fclose(log)) abort();
    if (fail) injected = 1;
    errno = saved_errno;
    return result;
}
