#define _GNU_SOURCE
#include <fcntl.h>
#include <linux/openat2.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Kernel failure keeps errno; 1 means a successful query did not prove a peer. */
int zxs_startup_peer_uid(int fd, uint32_t *uid) {
    struct ucred cred = {0};
    socklen_t bytes = sizeof(cred);
    _Static_assert(sizeof(cred.uid) == sizeof(*uid), "Linux peer UID width");
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &cred, &bytes) != 0) return -1;
    if (bytes != sizeof(cred) || cred.pid <= 0) return 1;
    *uid = cred.uid;
    return 0;
}

/* Member of the retained deployment/role directory, never a followed name. */
long zxs_startup_open_member(int parent, const char *name) {
    const struct open_how how = {
        .flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK,
        .resolve = RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV,
    };
    return syscall(SYS_openat2, parent, name, &how, sizeof(how));
}

#if defined(__x86_64__)
uint32_t zxs_startup_effective_gid(void) {
    _Static_assert(sizeof(gid_t) == sizeof(uint32_t), "Linux effective GID width");
    return getegid();
}

/* Apply permissions only to the previously pinned private socket inode. */
long zxs_startup_socket_mode(int fd) {
    return syscall(__NR_fchmodat2, fd, "", (mode_t)0660,
                   AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW);
}

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

#ifdef ZXS_HOST_ACTIVATION
long zxs_startup_quit_gateway(int pidfd) {
    return syscall(SYS_pidfd_send_signal, pidfd, SIGQUIT, (const siginfo_t *)NULL, 0U);
}

long zxs_startup_term_service(int pidfd) {
    return syscall(SYS_pidfd_send_signal, pidfd, SIGTERM, (const siginfo_t *)NULL, 0U);
}
#endif
#endif
