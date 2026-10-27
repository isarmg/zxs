/* Only the selected libtest child is preloaded. Observe libc fsync, not raw
 * syscalls or physical power loss. Existing target files are preinstalled. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static char targets[3][4096], parents[3][4096];
static int loaded, pending[3], injected;

static void load_targets(const char *root) {
    char path[4096];
    if (snprintf(path, sizeof(path), "%s/targets", root) >= (int)sizeof(path)) abort();
    FILE *input = fopen(path, "r");
    if (!input) abort();
    for (int i = 0; i < 3; i++) {
        if (!fgets(targets[i], sizeof(targets[i]), input)) abort();
        size_t size = strlen(targets[i]);
        if (!size || targets[i][size - 1] != '\n') abort();
        targets[i][size - 1] = '\0';
        strcpy(parents[i], targets[i]);
        char *slash = strrchr(parents[i], '/');
        if (!slash) abort();
        *slash = '\0';
    }
    if (fgetc(input) != EOF || ferror(input) || fclose(input)) abort();
    loaded = 1;
}

static void event(int member, const char *kind, const char *status) {
    FILE *output = fopen(getenv("ZXS_HISTORY_IO_LOG"), "a");
    if (!output) abort();
    if (fprintf(output, "%d %s %s\n", member, kind, status) < 0 || fclose(output)) abort();
}

int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    if (!real_fsync) abort();
    const char *root = getenv("ZXS_HISTORY_IO_ROOT");
    char marker[4096];
    if (!root) abort();
    if (snprintf(marker, sizeof(marker), "%s/observe", root) >= (int)sizeof(marker)) abort();
    if (access(marker, F_OK)) return real_fsync(fd);
    if (!loaded) load_targets(root);
    char descriptor[64], path[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size < 0) abort();
    path[size] = '\0';
    struct stat metadata;
    if (fstat(fd, &metadata)) abort();
    int member = -1;
    const char *kind = NULL;
    for (int i = 0; i < 3; i++) {
        if (S_ISREG(metadata.st_mode) && !strcmp(path, targets[i])) {
            member = i;
            kind = "file";
        } else if (S_ISDIR(metadata.st_mode) && pending[i] && !strcmp(path, parents[i])) {
            member = i;
            kind = "directory";
        }
    }
    if (member < 0) return real_fsync(fd);
    event(member, kind, "attempt");
    if (!injected && member == atoi(getenv("ZXS_HISTORY_IO_MEMBER")) &&
        !strcmp(kind, getenv("ZXS_HISTORY_IO_STAGE"))) {
        injected = 1;
        pending[member] = 0;
        event(member, kind, "injected-EIO");
        errno = EIO;
        return -1;
    }
    int result = real_fsync(fd);
    int saved_errno = errno;
    event(member, kind, result ? "failed" : "success");
    pending[member] = !result && !strcmp(kind, "file");
    errno = saved_errno;
    return result;
}
