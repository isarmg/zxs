/* Test-only syscall interposer, never linked into production.
 * cc -shared -fPIC -o publication_io_tails.so io_tails.c -ldl
 * Each test arms only its private fixture after setup. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int child(char *out, const char *directory, const char *name) {
    int size = snprintf(out, PATH_MAX, "%s/%s", directory, name);
    return size >= 0 && size < PATH_MAX;
}

static int touch(const char *path) {
    int file = open(path, O_WRONLY | O_CREAT | O_CLOEXEC, 0600);
    if (file < 0) return EIO;
    return close(file) == 0 ? 0 : EIO;
}

static int mutation_visible(const char *directory) {
    char marker[PATH_MAX], watched[PATH_MAX];
    if (!child(marker, directory, ".publication-watch")) return 0;
    int file = open(marker, O_RDONLY | O_CLOEXEC);
    if (file < 0) return 0;
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    ssize_t length = real_read(file, watched, sizeof(watched) - 1);
    close(file);
    if (length <= 0) return 0;
    watched[length] = '\0';
    int present = access(watched, F_OK) == 0;
    if (!child(marker, directory, ".publication-missing")) return 0;
    return access(marker, F_OK) == 0 ? !present : present;
}

static int gate(int fd, int reading) {
    char descriptor[64], path[PATH_MAX], marker[PATH_MAX];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0) return 0;
    path[length] = '\0';
    if (reading) {
        char *name = strrchr(path, '/');
        if (!name || strcmp(name, "/commit-proof.json") != 0) return 0;
        *name = '\0';
    } else {
        if (!mutation_visible(path)) return 0;
        if (!child(marker, path, ".publication-seen") || touch(marker)) return EIO;
    }
    if (!child(marker, path, reading ? ".publication-read-arm" : ".publication-arm")
        || unlink(marker) != 0) return 0;
    if (!child(marker, path, ".publication-entered") || touch(marker)) return EIO;
    if (!child(marker, path, ".publication-release")) return EIO;
    struct timespec start, now, pause = {0, 1000000};
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (access(marker, F_OK) != 0) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - start.tv_sec >= 30) return ETIMEDOUT;
        nanosleep(&pause, NULL);
    }
    if (!child(marker, path, ".publication-eio")) return EIO;
    return access(marker, F_OK) == 0 ? EIO : 0;
}

ssize_t read(int fd, void *buffer, size_t length) {
    int failure = gate(fd, 1);
    if (failure) { errno = failure; return -1; }
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return real_read(fd, buffer, length);
}

int fsync(int fd) {
    int failure = gate(fd, 0);
    if (failure) { errno = failure; return -1; }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}
