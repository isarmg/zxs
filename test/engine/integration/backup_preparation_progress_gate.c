/* Test-only descriptor-scoped gate for one real SYSTEM progress write.
 * Preload only the isolated child with ZXS_BP_PROGRESS_ROOT set. It writes the
 * actual bytes first, reports arrival, then waits at most 15 seconds for release.
 * No production hooks, fake Engine status or source/target modifications. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static pthread_mutex_t gate_mutex = PTHREAD_MUTEX_INITIALIZER;
static int claimed;

static int matches_progress(int fd, const void *buffer, size_t count, const char *root) {
    char descriptor[64], path[PATH_MAX], prefix[PATH_MAX];
    if (!root || root[0] != '/' || !memmem(buffer, count, "\"phase\"", 7)) return 0;
    int n = snprintf(prefix, sizeof(prefix), "%s/system/backup-preparation/.state-atomic-", root);
    if (n < 0 || (size_t)n >= sizeof(prefix)) return 0;
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, path, sizeof(path) - 1);
    if (size < 0 || (size_t)size >= sizeof(path) - 1) return 0;
    path[size] = '\0';
    return strncmp(path, prefix, (size_t)n) == 0;
}

ssize_t write(int fd, const void *buffer, size_t count) {
    int saved_errno = errno;
    const char *root = getenv("ZXS_BP_PROGRESS_ROOT");
    pthread_mutex_lock(&gate_mutex);
    if (claimed || !matches_progress(fd, buffer, count, root)) {
        pthread_mutex_unlock(&gate_mutex);
        errno = saved_errno;
        return syscall(SYS_write, fd, buffer, count);
    }
    claimed = 1;
    pthread_mutex_unlock(&gate_mutex);
    errno = saved_errno;
    ssize_t written = syscall(SYS_write, fd, buffer, count);
    int write_errno = errno;
    if (written <= 0) return written;
    char ready[PATH_MAX], pending[PATH_MAX], release[PATH_MAX], bytes[64];
    int n = snprintf(ready, sizeof(ready), "%s/progress-write-ready", root);
    int m = snprintf(release, sizeof(release), "%s/progress-write-release", root);
    int p = snprintf(pending, sizeof(pending), "%s/.progress-write-ready.tmp", root);
    if (n < 0 || (size_t)n >= sizeof(ready) || m < 0 || (size_t)m >= sizeof(release)
        || p < 0 || (size_t)p >= sizeof(pending)) {
        errno = ENAMETOOLONG;
        return -1;
    }
    int observed = open(pending, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (observed < 0) return -1;
    int length = snprintf(bytes, sizeof(bytes), "%zd\n", written);
    ssize_t reported = syscall(SYS_write, observed, bytes, (size_t)length);
    int closed = close(observed);
    if (reported != length || closed != 0) { errno = EIO; return -1; }
    if (rename(pending, ready) != 0) return -1;
    struct timespec start, now, pause = {0, 1000000};
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (access(release, F_OK) != 0) {
        if (errno != ENOENT) return -1;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - start.tv_sec >= 15) { errno = ETIMEDOUT; return -1; }
        nanosleep(&pause, NULL);
    }
    errno = write_errno;
    return written;
}
