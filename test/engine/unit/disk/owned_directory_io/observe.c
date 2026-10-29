/* Only isolated test children preload this probe. It observes libc fsync and
 * injects one real EIO after mkdir has taken effect; it is not a power-loss model. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int injected;

static void event(const char *phase, const char *kind, const char *path, const char *status) {
    const char *root = getenv("ZXS_DIRECTORY_IO_ROOT");
    char name[4096];
    if (snprintf(name, sizeof(name), "%s/events", root) >= (int)sizeof(name)) abort();
    FILE *log = fopen(name, "a");
    if (!log) abort();
    if (fprintf(log, "%s %s %s %s\n", phase, kind, path, status) < 0 || fclose(log)) abort();
}

int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    if (!real_fsync) abort();
    const char *root = getenv("ZXS_DIRECTORY_IO_ROOT");
    if (!root) return real_fsync(fd);
    char marker[4096];
    if (snprintf(marker, sizeof(marker), "%s/observe", root) >= (int)sizeof(marker)) abort();
    if (access(marker, F_OK)) return real_fsync(fd);
    char descriptor[64], path[4096];
    if (snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd) >= (int)sizeof(descriptor)) abort();
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0) abort();
    path[length] = '\0';
    size_t prefix = strlen(root);
    if (strncmp(path, root, prefix) || (path[prefix] && path[prefix] != '/')) return real_fsync(fd);
    const char *relative = path[prefix] ? path + prefix + 1 : ".";
    struct stat metadata;
    if (fstat(fd, &metadata)) abort();
    const char *kind = S_ISDIR(metadata.st_mode) ? "dir" : "file";
    char phase_path[4096], phase[32];
    if (snprintf(phase_path, sizeof(phase_path), "%s/phase", root) >= (int)sizeof(phase_path)) abort();
    FILE *input = fopen(phase_path, "r");
    if (!input || !fgets(phase, sizeof(phase), input) || fclose(input)) abort();
    if (!injected && !strcmp(phase, "first") && !strcmp(kind, "dir") &&
        !strcmp(relative, "metadata/objects")) {
        char child[4096];
        if (snprintf(child, sizeof(child), "%s/metadata/objects/publication", root) >= (int)sizeof(child)) abort();
        struct stat created;
        if (!stat(child, &created) && S_ISDIR(created.st_mode)) {
            injected = 1;
            event(phase, kind, relative, "injected-EIO");
            errno = EIO;
            return -1;
        }
    }
    int result = real_fsync(fd);
    int saved_errno = errno;
    event(phase, kind, relative, result ? "failed" : "success");
    errno = saved_errno;
    return result;
}
