/* Isolated test process only: deny reads of disk.json when its sibling
 * observation-read-error marker exists. No production hook or privilege change. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
ssize_t read(int fd, void *buffer, size_t count) {
    char descriptor[64], path[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size >= 0 && size < (ssize_t)sizeof(path) - 32) {
        path[size] = '\0';
        char *name = strrchr(path, '/');
        if (name && strcmp(name + 1, "disk.json") == 0) {
            strcpy(name + 1, "observation-read-error");
            if (access(path, F_OK) == 0) {
                fprintf(stderr, "\nZXS_OBSERVATION_SEAL_EACCES\n");
                errno = EACCES;
                return -1;
            }
        }
    }
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return real_read(fd, buffer, count);
}
