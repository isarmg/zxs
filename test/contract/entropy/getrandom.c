#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <unistd.h>

/* Only the explicitly launched test subprocess sees this interposer. */
ssize_t getrandom(void *buffer, size_t length, unsigned int flags) {
    const char *marker = getenv("ZXS_TEST_ENTROPY_FAIL_FILE");
    if (length != 0 && marker != NULL && access(marker, F_OK) == 0) {
        /* Failure may follow a partial write; callers must publish no ID. */
        memset(buffer, 0xab, length < 8 ? length : 8);
        errno = EIO;
        return -1;
    }
    ssize_t (*real_getrandom)(void *, size_t, unsigned int) = dlsym(RTLD_NEXT, "getrandom");
    if (real_getrandom == NULL) {
        errno = ENOSYS;
        return -1;
    }
    return real_getrandom(buffer, length, flags);
}
