/* Test-only Linux syscall interposer, never linked into a product target.
 * GitHub runner contract (from the repository root):
 * cc -std=c11 -Wall -Wextra -Werror -shared -fPIC \
 *   -o /tmp/journal_io_tails.so test/engine/unit/journal/journal_io_tails.c -ldl
 * LD_PRELOAD=/tmp/journal_io_tails.so cargo test --locked -p storage-engine \
 *   --lib --features fault-injection disk::admission::journal::blocked_io_tests \
 *   -- --ignored --test-threads=1
 * This pauses a real caller's libc syscall boundary, not a kernel/device stall.
 * Each gate is one-shot, path-scoped, released on test unwind, and time bounded.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int child_path(char *out, size_t capacity, const char *directory, const char *name) {
    int size = snprintf(out, capacity, "%s/.journal-tail-%s", directory, name);
    return size >= 0 && (size_t)size < capacity;
}

static int ends_with(const char *path, const char *suffix) {
    size_t length = strlen(path), tail = strlen(suffix);
    return length >= tail && strcmp(path + length - tail, suffix) == 0;
}

static int gate(int fd, const char *operation, int directory) {
    char descriptor[64], path[PATH_MAX], marker[PATH_MAX], arm[64];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0 || (size_t)length == sizeof(path) - 1) return 0;
    path[length] = '\0';
    if (directory) {
        if (!ends_with(path, "/zxs-journal-syscall-tail/metadata/application-log")) return 0;
    } else {
        if (!ends_with(path, "/zxs-journal-syscall-tail/metadata/application-log/journal.bin")) return 0;
        *strrchr(path, '/') = '\0';
    }
    snprintf(arm, sizeof(arm), "arm-%s", operation);
    if (!child_path(marker, sizeof(marker), path, arm) || unlink(marker) != 0) return 0;
    if (!child_path(marker, sizeof(marker), path, "entered")) return EIO;
    int signal = open(marker, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (signal < 0) return EIO;
    close(signal);
    if (!child_path(marker, sizeof(marker), path, "release")) return EIO;
    struct timespec start, now, pause = {0, 1000000};
    if (clock_gettime(CLOCK_MONOTONIC, &start) != 0) return EIO;
    while (access(marker, F_OK) != 0) {
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return EIO;
        if (now.tv_sec - start.tv_sec >= 30) return ETIMEDOUT;
        nanosleep(&pause, NULL);
    }
    if (!child_path(marker, sizeof(marker), path, "eio")) return EIO;
    return access(marker, F_OK) == 0 ? EIO : 0;
}

ssize_t read(int fd, void *buffer, size_t length) {
    int failure = gate(fd, "read", 0);
    if (failure) { errno = failure; return -1; }
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return real_read(fd, buffer, length);
}

ssize_t write(int fd, const void *buffer, size_t length) {
    int failure = gate(fd, "write", 0);
    if (failure) { errno = failure; return -1; }
    ssize_t (*real_write)(int, const void *, size_t) = dlsym(RTLD_NEXT, "write");
    return real_write(fd, buffer, length);
}

int fdatasync(int fd) {
    int failure = gate(fd, "data-sync", 0);
    if (failure) { errno = failure; return -1; }
    int (*real_fdatasync)(int) = dlsym(RTLD_NEXT, "fdatasync");
    return real_fdatasync(fd);
}

int fsync(int fd) {
    int failure = gate(fd, "directory-sync", 1);
    if (failure) { errno = failure; return -1; }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}

int ftruncate(int fd, off_t length) {
    int failure = gate(fd, "truncate", 0);
    if (failure) { errno = failure; return -1; }
    int (*real_ftruncate)(int, off_t) = dlsym(RTLD_NEXT, "ftruncate");
    return real_ftruncate(fd, length);
}

int ftruncate64(int fd, off64_t length) {
    int failure = gate(fd, "truncate", 0);
    if (failure) { errno = failure; return -1; }
    int (*real_ftruncate64)(int, off64_t) = dlsym(RTLD_NEXT, "ftruncate64");
    return real_ftruncate64(fd, length);
}

int close(int fd) {
    /* Always close the real FD, including on timeout: never inject a leak or
     * pretend that a failed close proves the original descriptor was retired. */
    int saved = errno;
    int failure = gate(fd, "close-file", 0);
    if (!failure) failure = gate(fd, "close-directory", 1);
    int (*real_close)(int) = dlsym(RTLD_NEXT, "close");
    errno = saved;
    int result = real_close(fd);
    if (failure) { errno = failure; return -1; }
    return result;
}
