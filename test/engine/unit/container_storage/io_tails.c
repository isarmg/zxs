/* Test-only syscall interposer, never linked into a production target.
 * Compile: cc -shared -fPIC -o io_tails.so io_tails.c -ldl
 * Tests arm a private directory only after their fixture has been created.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int child_path(char *out, size_t capacity, const char *directory, const char *name) {
    int size = snprintf(out, capacity, "%s/%s", directory, name);
    return size >= 0 && (size_t)size < capacity;
}

static int gate(int fd, int reading) {
    char descriptor[64], path[PATH_MAX], marker[PATH_MAX];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0) return 0;
    path[length] = '\0';
    char *name = strrchr(path, '/');
    if (!name) return 0;
    if (reading) {
        if (strcmp(name, "/body") != 0) return 0;
        *name = '\0';
        name = strrchr(path, '/');
        if (!name || strcmp(name, "/zxs-tail-hash") != 0) return 0;
    } else {
        if (strcmp(name, "/zxs-tail-rename-source") != 0
            && strcmp(name, "/zxs-tail-unlink-source") != 0
            && strcmp(name, "/zxs-tail-copy-content") != 0) return 0;
        if (strcmp(name, "/zxs-tail-copy-content") == 0) {
            if (!child_path(marker, sizeof(marker), path, "body") || access(marker, F_OK) != 0) return 0;
        }
    }
    if (!child_path(marker, sizeof(marker), path, ".io-arm") || unlink(marker) != 0) return 0;
    if (!child_path(marker, sizeof(marker), path, ".io-entered")) return EIO;
    int signal = open(marker, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (signal < 0) return EIO;
    close(signal);
    if (!child_path(marker, sizeof(marker), path, ".io-release")) return EIO;
    struct timespec start, now, pause = {0, 1000000};
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (access(marker, F_OK) != 0) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - start.tv_sec >= 30) return ETIMEDOUT;
        nanosleep(&pause, NULL);
    }
    if (!child_path(marker, sizeof(marker), path, ".io-eio")) return EIO;
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
