/* SPDX-License-Identifier: Apache-2.0
 * Fixed Linux process-control channel for two binaries started by the same
 * trusted supervisor. Not a listener, password interface or login token. */
#ifndef ZXS_SMB_CONTROL_H
#define ZXS_SMB_CONTROL_H
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
/* Stable public service component, not a hostname, FILE path or grant. */
int zxs_smb_service_id(const uint8_t *s, size_t n, uint8_t id[16]);
int zxs_smb_direct_service(const char *name, int browseable, int native_pipes, int host_dfs);
#define ZXS_SMB_BIND_LEN 128
#define ZXS_SMB_READY_LEN 277
int zxs_smb_ctl_claim(int fd);
int zxs_smb_ctl_read(int fd, uint8_t *buf, size_t size);
int zxs_smb_ctl_write(int fd,const uint8_t *buf,size_t size);
int zxs_smb_ctl_live(int fd);
int zxs_smb_ctl_hello(int fd,const uint8_t *bound);
int zxs_smb_ctl_bound(int fd,uint8_t *bound);
int zxs_smb_ctl_life(int pipe_reader,int timeout);
#define ZXS_SMB_MAX_ATTEMPTS 32
#define ZXS_SMB_MAX_SESSIONS 64
/* No network event-loop call waits for storage. Submission and receive
 * both use MSG_DONTWAIT. Only the original readiness callback receives. */
int zxs_smb_try_open(int original_control,uint64_t net,uint64_t sk,int64_t *started_msec);
int zxs_smb_try_read(int attempt,int64_t started_msec,uint8_t challenge[8]);
int zxs_smb_try_current(int attempt,int64_t started_msec);
/* Exact dedicated Debian kernel origins, never a principal or reusable token. */
int zxs_smb_sender_isolated(void);
int zxs_smb_process_live(int pidfd);
int zxs_smb_process_claim(int pidfd);
int zxs_smb_same_process(int left,int right);
int zxs_smb_attempt_peer(int endpoint,int sender_pidfd);
int zxs_smb_origin_enable(int control);
int zxs_smb_dispatch(int control,pid_t direct_child,uint64_t net,uint64_t sk);
int zxs_smb_origin_recv(int control,uint64_t tcp[2],int *sender,int *right);
int zxs_smb_try_issue(int attempt,const uint8_t challenge[8]);
/* Nonsecret last-generation cut on the one original attempt. It is not
 * password authentication, and is never accepted on the master socket. */
#define ZXS_SMB_CUT_BYTES 149
struct zxs_smb_generation {
    uint8_t pool[16],account[16],credential_op[16],sid[28];
    uint64_t generation;
    char login[65];
};
int zxs_smb_public_generation(const uint8_t id[ZXS_SMB_CUT_BYTES],struct zxs_smb_generation *out);
int zxs_smb_cut_ask(int fd,int64_t started,const uint8_t challenge[8]);
int zxs_smb_cut_take(int fd,const uint8_t challenge[8]);
int zxs_smb_cut_reply(int fd,const uint8_t challenge[8],const uint8_t identity[ZXS_SMB_CUT_BYTES]);
int zxs_smb_cut_read(int fd,int64_t started,const uint8_t challenge[8],struct zxs_smb_generation *out);
/* Business exchange on the same private endpoint only after native final
 * session_update. These functions carry no authentication material. */
int zxs_smb_active_send(int fd,const uint8_t challenge[8]);
int zxs_smb_active_take(int fd,const uint8_t challenge[8]);
struct zxs_smb_tree_cut {
    uint8_t share[16],volume[16],setting[16];
    uint64_t revision;
    uint8_t mode,pool[16],volume_ordinal;
    int root_fd; /* local only: owned by tcon; never serialized */
};
#define ZXS_SMB_TREE_CUT_BYTES 74
#define ZXS_SMB_VIRTUAL_ROOT "/zxs-root"
int zxs_smb_root_wait(int tree,int session,int timeout);
int zxs_smb_root_release(struct zxs_smb_tree_cut *owned);
int zxs_smb_tree_ask(int session,const uint8_t share[16]);
int zxs_smb_tree_take(int session,int worker,uint8_t share[16]);
int zxs_smb_tree_reply(int fd,const uint8_t decision[ZXS_SMB_TREE_CUT_BYTES]);
int zxs_smb_tree_read(int fd,const uint8_t share[16],struct zxs_smb_tree_cut *out);
/* events: bit 1 readable, bit 2 terminal. Index 0 control, 1 lifetime,
 * then the same bounded list of independent attempt sockets. */
int zxs_smb_try_wait(int ctl,int lifetime,const int *attempt,size_t n,uint8_t *events);
int zxs_smb_ctl_drain(int fd);
int zxs_smb_bound_fd(int fd);
#endif
