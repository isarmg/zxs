/* Linux ABI for the original approved gateway and its optional launcher. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <linux/magic.h>
#include <poll.h>
#include <stdbool.h>
#include <sys/socket.h>
#include <sys/vfs.h>
#include <unistd.h>

/* Some build-host Linux headers predate PIDFS; the Linux UAPI magic is
   fixed independently of the running kernel or the native statfs layout. */
#ifndef PID_FS_MAGIC
#define PID_FS_MAGIC 0x50494446
#endif

static const int receipt_seals = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;

enum { READY_SLOT = 6 };

int zxs_gateway_duplicate(int fd) {
    return fcntl(fd, F_DUPFD_CLOEXEC, READY_SLOT + 1);
}

int zxs_gateway_read_flags(bool directory) {
    return O_NOFOLLOW | O_CLOEXEC | (directory ? O_DIRECTORY : O_NONBLOCK);
}

/* A negative result preserves fstatfs errno; a zero result is not PIDFS. */
int zxs_gateway_pidfs(int fd) {
    struct statfs info = {0};
    if (fstatfs(fd, &info) != 0) return -1;
    return info.f_type == PID_FS_MAGIC;
}

int zxs_gateway_is_live(int fd) {
    struct pollfd item = { .fd = fd, .events = POLLIN, .revents = 0 };
    int result = poll(&item, 1, 0);
    if (result < 0) return result;
    return result == 0 && item.revents == 0;
}

/* As before, an unavailable fact is a refusal, not a separate error case. */
bool zxs_gateway_has_receipt_seals(int fd) {
    int seals = fcntl(fd, F_GET_SEALS);
    return seals >= 0 && (seals & receipt_seals) == receipt_seals;
}

bool zxs_gateway_is_readonly_file(int fd) {
    int flags = fcntl(fd, F_GETFL);
    return flags >= 0 && (flags & (O_ACCMODE | O_PATH)) == O_RDONLY;
}

/* The original root launcher created the stream before fork. A named
   endpoint is rejected separately by the Rust UnixStream address checks. */
int zxs_gateway_ready_peer(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) return -1;
    if ((flags & (O_ACCMODE | O_NONBLOCK | O_PATH)) != (O_RDWR | O_NONBLOCK)) return 0;
    int kind = 0;
    socklen_t size = sizeof(kind);
    if (getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &kind, &size) != 0) return -1;
    if (size != sizeof(kind) || kind != AF_UNIX) return 0;
    size = sizeof(kind);
    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &kind, &size) != 0) return -1;
    if (size != sizeof(kind) || kind != SOCK_STREAM) return 0;
    struct ucred peer = {0};
    size = sizeof(peer);
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &size) != 0) return -1;
    return size == sizeof(peer) && peer.uid == 0 && peer.pid > 0 && peer.pid == getppid();
}

#ifdef ZXS_HOST_ACTIVATION
#include <errno.h>
#include <grp.h>
#include <limits.h>
#include <linux/capability.h>
#include <linux/close_range.h>
#include <signal.h>
#include <stdint.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/types.h>

_Static_assert(_Generic((pid_t)0, int: 1, default: 0), "native pid_t is the checked int");
_Static_assert(_Generic((uid_t)0, uint32_t: 1, default: 0), "native uid_t is uint32_t");
_Static_assert(_Generic((gid_t)0, uint32_t: 1, default: 0), "native gid_t is uint32_t");

int zxs_gateway_create_memfd(const char *label) {
    return memfd_create(label, MFD_CLOEXEC | MFD_ALLOW_SEALING);
}

int zxs_gateway_seal_memfd(int fd) {
    return fcntl(fd, F_ADD_SEALS, receipt_seals);
}

long zxs_gateway_open_pidfd(int owned_child) {
    return syscall(SYS_pidfd_open, (pid_t)owned_child, 0u);
}

int zxs_gateway_lock_config(int fd) {
    return flock(fd, LOCK_EX | LOCK_NB);
}

int zxs_gateway_quit_owned_child(int owned_child) {
    int status = kill((pid_t)owned_child, SIGQUIT);
    return status == 0 || errno == ESRCH ? 0 : status;
}

int zxs_gateway_open_service_record(int state) {
    return openat(state, "server.v1", O_RDONLY | zxs_gateway_read_flags(false));
}

/* Only used after fork. No allocation, retries or work after the first
   failure. Keep the original close-on-exec isolation before remapping. */
static long protect_inherited_fds(void) {
    return syscall(SYS_close_range, 3u, UINT_MAX, CLOSE_RANGE_CLOEXEC);
}

int zxs_gateway_prepare_nginx_exec(int config) {
    if (protect_inherited_fds() != 0 || fcntl(config, F_SETFD, 0) < 0) return -1;
    return 0;
}

int zxs_gateway_prepare_service_exec(int pidfd, int config, int receipt, int ready,
                                    uint32_t uid, uint32_t gid) {
    if (protect_inherited_fds() != 0 ||
        dup2(pidfd, 3) != 3 ||
        dup2(config, 4) != 4 ||
        dup2(receipt, 5) != 5 ||
        dup2(ready, READY_SLOT) != READY_SLOT ||
        prctl(PR_SET_NO_NEW_PRIVS, 1ul, 0ul, 0ul, 0ul) != 0 ||
        prctl(PR_SET_KEEPCAPS, 0ul, 0ul, 0ul, 0ul) != 0 ||
        prctl(PR_CAP_AMBIENT, (unsigned long)PR_CAP_AMBIENT_CLEAR_ALL, 0ul, 0ul, 0ul) != 0 ||
        setgroups(0, NULL) != 0 ||
        setresgid((gid_t)gid, (gid_t)gid, (gid_t)gid) != 0 ||
        setresuid((uid_t)uid, (uid_t)uid, (uid_t)uid) != 0) return -1;

    struct __user_cap_header_struct header = { .version = _LINUX_CAPABILITY_VERSION_3, .pid = 0 };
    struct __user_cap_data_struct capabilities[_LINUX_CAPABILITY_U32S_3] = {{0}};
    return syscall(SYS_capset, &header, capabilities) == 0 ? 0 : -1;
}
#endif
