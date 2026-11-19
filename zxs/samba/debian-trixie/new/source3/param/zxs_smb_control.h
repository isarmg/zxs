/* SPDX-License-Identifier: Apache-2.0
 * Fixed Linux process-control channel for two binaries started by the same
 * trusted supervisor. Not a listener, password interface or login token. */
#ifndef ZXS_SMB_CONTROL_H
#define ZXS_SMB_CONTROL_H
#include <stddef.h>
#include <stdint.h>
#define ZXS_SMB_BIND_LEN 128
#define ZXS_SMB_READY_LEN 212
int zxs_smb_ctl_claim(int fd);
int zxs_smb_ctl_read(int fd, uint8_t *buf, size_t size);
int zxs_smb_ctl_write(int fd,const uint8_t *buf,size_t size);
int zxs_smb_ctl_live(int fd);
int zxs_smb_ctl_hello(int fd,const uint8_t *bound);
int zxs_smb_ctl_bound(int fd,uint8_t *bound);
int zxs_smb_ctl_life(int pipe_reader,int timeout);
int zxs_smb_ctl_drain(int fd);
int zxs_smb_bound_fd(int fd);
#endif
