/* Isolated regression interposer: mutate only the selected temporary fixture
 * after the old publication's successful File fsync, before returning to Rust. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int mutated;

static void record(const char *event) {
    FILE *log = fopen(getenv("ZXS_AUTHORITY_MUTATION_LOG"), "a");
    if (!log || fprintf(log, "%s\n", event) < 0 || fclose(log)) abort();
}

int fsync(int fd) {
    int (*real_fsync)(int) = dlsym(RTLD_NEXT, "fsync");
    int result = real_fsync(fd);
    int saved_errno = errno;
    const char *trigger = getenv("ZXS_AUTHORITY_MUTATION_TRIGGER");
    const char *root = getenv("ZXS_AUTHORITY_TEST_DISK");
    if (result || mutated || !trigger || !root || access(trigger, F_OK)) {
        errno = saved_errno;
        return result;
    }
    char descriptor[64], opened[4096], barrier[4096], selected[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(descriptor, opened, sizeof(opened) - 1);
    if (size < 0) abort();
    opened[size] = '\0';
    snprintf(barrier, sizeof(barrier), "%s/publications/00000000000000000001.json", root);
    if (strcmp(opened, barrier)) {
        errno = saved_errno;
        return result;
    }
    FILE *selection = fopen(getenv("ZXS_AUTHORITY_MUTATION_SELECTION"), "r");
    if (!selection) abort();
    size_t count = fread(selected, 1, sizeof(selected) - 1, selection);
    if (ferror(selection) || !feof(selection) || fclose(selection) || !count) abort();
    selected[count] = '\0';
    if (strncmp(selected, root, strlen(root)) || selected[strlen(root)] != '/') abort();
    mutated = 1;
    const char *kind = getenv("ZXS_AUTHORITY_MUTATION_KIND");
    if (!strcmp(kind, "rollback") || !strcmp(kind, "missing-body")) {
        if (unlink(selected)) abort();
        record("removed-selected-file");
    } else {
        int target = open(selected, O_RDWR | O_CLOEXEC | O_NOFOLLOW);
        struct stat before, after;
        unsigned char byte;
        if (target < 0 || fstat(target, &before) || !S_ISREG(before.st_mode)
            || before.st_nlink != 1 || pread(target, &byte, 1, 0) != 1) abort();
        byte ^= 1;
        if (pwrite(target, &byte, 1, 0) != 1 || real_fsync(target)
            || fstat(target, &after) || before.st_dev != after.st_dev
            || before.st_ino != after.st_ino || before.st_size != after.st_size
            || close(target)) abort();
        record("same-inode-rewrite");
    }
    errno = saved_errno;
    return result;
}
