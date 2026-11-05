/* Test-only interposer, never linked into the product. Linux GitHub runner:
 * cc -shared -fPIC -Wall -Wextra -Werror -o /tmp/repair_io_tails.so \
 *   test/engine/unit/repair/io_tails.c -ldl
 * LD_PRELOAD=/tmp/repair_io_tails.so cargo test --locked --offline \
 *   -p storage-engine --lib repair::tests::io_tail_tests -- --ignored --test-threads=1
 * LD_PRELOAD=/tmp/repair_io_tails.so cargo test --locked --offline \
 *   -p storage-engine --lib degraded_recovery::tests::classification_ \
 *   -- --ignored --test-threads=1
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

static __thread int inside;

static int child(char *out, const char *directory, const char *name) {
    int size = snprintf(out, PATH_MAX, "%s/%s", directory, name);
    return size >= 0 && size < PATH_MAX;
}

static int write_marker(const char *path, const char *value, int exclusive) {
    int file = open(path, O_WRONLY | O_CREAT | O_CLOEXEC | (exclusive ? O_EXCL : 0), 0600);
    if (file < 0) return errno;
    size_t length = strlen(value);
    int result = write(file, value, length) == (ssize_t)length ? 0 : EIO;
    if (close(file) != 0) result = EIO;
    return result;
}

static int watch_root(const char *path, char *root, char *watched) {
    char marker[PATH_MAX];
    strcpy(root, path);
    while (1) {
        if (!child(marker, root, ".repair-tail-watch")) return 0;
        int file = open(marker, O_RDONLY | O_CLOEXEC);
        if (file >= 0) {
            ssize_t length = read(file, watched, PATH_MAX - 1);
            close(file);
            if (length <= 0) return 0;
            watched[length] = '\0';
            return 1;
        }
        char *slash = strrchr(root, '/');
        if (!slash || slash == root) return 0;
        *slash = '\0';
    }
}

static int gate(int fd, const char *kind) {
    char descriptor[64], path[PATH_MAX], root[PATH_MAX], watched[PATH_MAX];
    char marker[PATH_MAX], name[80];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, path, sizeof(path) - 1);
    if (length < 0) return 0;
    path[length] = '\0';
    if (!watch_root(path, root, watched)) return 0;
    if (strcmp(kind, "sync") == 0) {
        if (access(watched, F_OK) == 0 || errno != ENOENT) return 0;
        /* An optional exact target lets cancellation tests stop at the final
         * directory barrier rather than an earlier regular-file barrier. */
        if (!child(marker, root, ".repair-tail-sync-target")) return EIO;
        int selected = open(marker, O_RDONLY | O_CLOEXEC);
        if (selected >= 0) {
            char target[PATH_MAX];
            ssize_t count = read(selected, target, sizeof(target) - 1);
            int closed = close(selected);
            if (count <= 0 || closed != 0) return EIO;
            target[count] = '\0';
            if (strcmp(path, target) != 0) return 0;
        } else if (errno != ENOENT) {
            return errno;
        }
        if (!child(marker, root, ".repair-tail-seen-first")) return EIO;
        int written = write_marker(marker, path, 1);
        if (written == 0) {
            kind = "first";
        } else if (written == EEXIST) {
            if (!child(marker, root, ".repair-tail-seen-second")) return EIO;
            written = write_marker(marker, path, 1);
            if (written == EEXIST) return 0;
            if (written != 0) return written;
            kind = "second";
        } else {
            return written;
        }
    } else if (strcmp(path, watched) != 0) {
        return 0;
    }
    if (strcmp(kind, "close") == 0) {
        int flags = fcntl(fd, F_GETFL);
        if (flags < 0) return errno;
        if (flags & O_PATH) return 0;
        if (!child(marker, root, ".repair-tail-skip-close")) return EIO;
        if (unlink(marker) == 0) return 0;
        if (errno != ENOENT) return errno;
    }
    snprintf(name, sizeof(name), ".repair-tail-arm-%s", kind);
    if (!child(marker, root, name) || unlink(marker) != 0) return 0;
    if (!child(marker, root, ".repair-tail-entered")) return EIO;
    int written = write_marker(marker, "", 0);
    if (written != 0) return written;
    if (!child(marker, root, ".repair-tail-release")) return EIO;
    struct timespec start, now, pause = {0, 1000000};
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (access(marker, F_OK) != 0) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - start.tv_sec >= 30) return ETIMEDOUT;
        nanosleep(&pause, NULL);
    }
    if (!child(marker, root, ".repair-tail-eio")) return EIO;
    return access(marker, F_OK) == 0 ? EIO : 0;
}

static int guarded_gate(int fd, const char *kind) {
    if (inside) return 0;
    inside = 1;
    int result = gate(fd, kind);
    inside = 0;
    return result;
}

ssize_t read(int fd, void *buffer, size_t length) {
    int failure = guarded_gate(fd, "read");
    if (failure) { errno = failure; return -1; }
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    return real_read(fd, buffer, length);
}

int fsync(int fd) {
    int failure = guarded_gate(fd, "sync");
    if (failure) { errno = failure; return -1; }
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    return real_fsync(fd);
}

int close(int fd) {
    int failure = guarded_gate(fd, "close");
    int (*real_close)(int) = dlsym(RTLD_NEXT, "close");
    int result = real_close(fd);
    if (failure) { errno = failure; return -1; }
    return result;
}
