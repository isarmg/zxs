/* Isolated libtest interposer; no fault hook is linked into the product. */
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int recovery;
static int delivered;

static struct dirent64 *next_entry(DIR *directory, int saved_errno) {
    struct dirent64 *(*real_readdir)(DIR *) = dlsym(RTLD_NEXT, "readdir64");
    /* Path probes must not turn a successful end-of-directory into ENOENT. */
    errno = saved_errno;
    return real_readdir(directory);
}

struct dirent64 *readdir64(DIR *directory) {
    int saved_errno = errno;
    const char *root = getenv("ZXS_TELEMETRY_ENUM_ROOT");
    char marker[4096], descriptor[64], path[4096];
    if (!root) return next_entry(directory, saved_errno);
    snprintf(marker, sizeof(marker), "%s/.inject", root);
    if (access(marker, F_OK)) return next_entry(directory, saved_errno);
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", dirfd(directory));
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size < 0) return next_entry(directory, saved_errno);
    path[size] = '\0';
    if (strcmp(path, root)) return next_entry(directory, saved_errno);
    /* Maintenance must complete; fail recovery only after a real cache entry. */
    if (recovery && delivered) {
        recovery = delivered = 0;
        fprintf(stderr, "ZXS_TELEMETRY_ENUM_EIO_AFTER_ENTRY\n");
        errno = EIO;
        return NULL;
    }
    struct dirent64 *entry = next_entry(directory, saved_errno);
    if (!entry) recovery = 1;
    else if (recovery && !strncmp(entry->d_name, "recent-", 7)) delivered = 1;
    return entry;
}
