/* Test-only syscall interposer, never linked into production.
 * cc -shared -fPIC -o maintenance_io_tails.so io_tails.c -ldl
 * Only a test's armed payload parent is gated, after its selected name is absent. */
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

static int payload_absent(const char *directory) {
    char marker[PATH_MAX], payload[PATH_MAX];
    if (!child(marker, directory, ".cleanup-watch")) return 0;
    int file = open(marker, O_RDONLY | O_CLOEXEC);
    if (file < 0) return 0;
    ssize_t length = read(file, payload, sizeof(payload) - 1);
    close(file);
    if (length <= 0) return 0;
    payload[length] = '\0';
    return access(payload, F_OK) != 0 && errno == ENOENT;
}

static int gate(int fd) {
    char descriptor[64], path[PATH_MAX], marker[PATH_MAX];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0) return 0;
    path[length] = '\0';
    if (!payload_absent(path)) return 0;
    if (!child(marker, path, ".cleanup-arm") || unlink(marker) != 0) return 0;
    if (!child(marker, path, ".cleanup-entered")) return EIO;
    int signal = open(marker, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (signal < 0) return EIO;
    close(signal);
    if (!child(marker, path, ".cleanup-release")) return EIO;
    struct timespec start, now, pause = {0, 1000000};
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (access(marker, F_OK) != 0) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - start.tv_sec >= 30) return ETIMEDOUT;
        nanosleep(&pause, NULL);
    }
    if (!child(marker, path, ".cleanup-enoent")) return EIO;
    if (access(marker, F_OK) == 0) return ENOENT;
    if (!child(marker, path, ".cleanup-eio")) return EIO;
    return access(marker, F_OK) == 0 ? EIO : 0;
}

int fsync(int fd) {
    int failure = gate(fd);
    if (failure) { errno = failure; return -1; }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}
