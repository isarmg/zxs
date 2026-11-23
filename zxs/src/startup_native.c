#define _GNU_SOURCE
#include <fcntl.h>
#include <linux/openat2.h>
#include <signal.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Member of the retained deployment/role directory, never a followed name. */
long zxs_startup_open_member(int parent, const char *name) {
    const struct open_how how = {
        .flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK,
        .resolve = RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV,
    };
    return syscall(SYS_openat2, parent, name, &how, sizeof(how));
}

#if defined(__x86_64__)
int zxs_startup_install_stop_handlers(void (*handler)(int)) {
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = handler;
    int rc = sigemptyset(&action.sa_mask);
    if (rc != 0) return rc;
    rc = sigaction(SIGTERM, &action, NULL);
    if (rc != 0) return rc;
    return sigaction(SIGINT, &action, NULL);
}
#endif
