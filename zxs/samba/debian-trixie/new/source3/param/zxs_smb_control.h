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
#define ZXS_SMB_READY_LEN 212
int zxs_smb_ctl_claim(int fd);
int zxs_smb_ctl_read(int fd, uint8_t *buf, size_t size);
int zxs_smb_ctl_write(int fd,const uint8_t *buf,size_t size);
int zxs_smb_ctl_live(int fd);
int zxs_smb_ctl_hello(int fd,const uint8_t *bound);
int zxs_smb_ctl_bound(int fd,uint8_t *bound);
int zxs_smb_ctl_life(int pipe_reader,int timeout);
#define ZXS_SMB_MAX_ATTEMPTS 32
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
/* events: bit 1 readable, bit 2 terminal. Index 0 control, 1 lifetime,
 * then the same bounded list of independent attempt sockets. */
int zxs_smb_try_wait(int ctl,int lifetime,const int *attempt,size_t n,uint8_t *events);
int zxs_smb_ctl_drain(int fd);
int zxs_smb_bound_fd(int fd);
#endif
