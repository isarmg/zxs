/* SPDX-License-Identifier: Apache-2.0
 * Fixed Linux process-control channel for two binaries started by the same
 * trusted supervisor. Not a listener, password interface or login token. */
#ifndef ZXS_SMB_CONTROL_H
#define ZXS_SMB_CONTROL_H
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
/* Stable public service component, not a hostname, FILE path or grant. */
int zxs_smb_namespace_name(const char *name);
#define ZXS_SMB_BIND_LEN 128
#define ZXS_SMB_READY_LEN 277
int zxs_smb_ctl_claim(int fd);
int zxs_smb_ctl_read(int fd, uint8_t *buf, size_t size);
int zxs_smb_ctl_write(int fd,const uint8_t *buf,size_t size);
#define ZXS_PLAN_SLOT 17631u
#define ZXS_PLAN_OFFER (18u+ZXS_PLAN_SLOT)
#define ZXS_PLAN_ACK 48u
int zxs_smb_plan_send(int fd,const uint8_t *plan,size_t n);
int zxs_smb_plan_take(int fd,uint8_t plan[ZXS_PLAN_SLOT]);
int zxs_smb_plan_reply(int fd,const uint8_t sha[32]);
int zxs_smb_plan_ack(int fd,const uint8_t sha[32]);
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
    uint8_t directory[16],directory_op[16]; /* only exact ordinary child proof; zero at top / R */
    int root_fd; /* local only: owned by FSP/tcon; never serialized */
    int parent_fd; /* real authorizing folder stays live while a child is held */
    bool owns_parent;
};
#define ZXS_SMB_TREE_CUT_BYTES 74
#define ZXS_SMB_VIRTUAL_ROOT "/zxs-root"
/* Single borrowed tree, bounded private request. Cursors are ordering keys only. */
#define ZXS_DIR_ROWS 16
#define ZXS_DIR_NAME 255
#define ZXS_FILE_NAME 222
#define ZXS_DIR_SLOT (1+ZXS_DIR_NAME+8+8)
#define ZXS_DIR_PACKET (16+1+ZXS_DIR_ROWS*ZXS_DIR_SLOT)
int zxs_smb_root_wait(int tree,int session,int timeout);
int zxs_dir_query(int tree,unsigned limit,const uint8_t *after,size_t n);
int zxs_dir_take(int tree,int worker,uint8_t *limit,uint8_t cursor[ZXS_DIR_NAME+1]);
#define ZXS_NS_OPEN 253
#define ZXS_NS_WRITE 252
#define ZXS_NS_CAPACITY_RW 242u
#define ZXS_NS_CAPACITY_RO 241u
#define ZXS_NS_CAP_READY 0u
#define ZXS_NS_CAP_NO_MEDIA 1u
#define ZXS_NS_CAP_BUSY 2u
struct zxs_ns_space {uint64_t total,available,free;uint8_t status;};
int zxs_ns_capacity_query(int tree,bool may_write);
int zxs_ns_capacity_reply(int fd,uint8_t status,uint64_t total,uint64_t available,uint64_t free_bytes);
int zxs_ns_capacity_read(int fd,struct zxs_ns_space *out);
/* Private only; a fresh original FILE operation reply socket, not a
 * native SMB packet or an independent business authority. */
#define ZXS_XFER_DELETE 1u
#define ZXS_XFER_RESTORE 2u
#define ZXS_XFER_PROBE 3u
#define ZXS_XFER_RENAME 4u
#define ZXS_XFER_MOVE 5u
#define ZXS_XFER_COPY_MOVE 6u
#define ZXS_XFER_ASK (20+ZXS_DIR_NAME+ZXS_FILE_NAME)
#define ZXS_XFER_RESULT 64u
#define ZXS_XFER_BUSY 1u
#define ZXS_XFER_FULL 2u
#define ZXS_XFER_COLLISION 3u
#define ZXS_XFER_NOT_FOUND 4u
#define ZXS_XFER_DENIED 5u
#define ZXS_XFER_UNAVAILABLE 6u
#define ZXS_XFER_UNKNOWN 7u
#define ZXS_XFER_UNSUPPORTED 8u
int zxs_xfer_query(int member,uint8_t kind,const uint8_t *folder,size_t f,const uint8_t *name,size_t n,const uint8_t mark[16]);
int zxs_member_xfer_pending(int member);
int zxs_xfer_take(int member,int worker,uint8_t *kind,uint8_t folder[ZXS_DIR_NAME+1],uint8_t name[ZXS_FILE_NAME+1],uint8_t mark[16],uint16_t *parent_len,uint8_t *parents);
int zxs_xfer_reply(int fd,uint8_t status,const uint8_t file[16],const uint8_t op[16]);
int zxs_xfer_read(int fd,const uint8_t expected[16],uint8_t *status,uint8_t receipt[16]);
#define ZXS_OD_PATH 243
#define ZXS_OD_PATH_MAX 4096
#define ZXS_OD_PATH_REQUEST (19+ZXS_OD_PATH_MAX)
#define ZXS_OD_PATH_RESULT (17+ZXS_SMB_TREE_CUT_BYTES+32)
#define ZXS_OD_PATH_ACK 48
#define ZXS_OD_SELECT_KIND 4u
#define ZXS_OD_FILE_KIND 7u
bool zxs_odir_component(const char *name,size_t n,size_t limit);
bool zxs_odir_relative(const char *path,bool directory);
int zxs_odir_query(int fd,uint8_t mode,const uint8_t *name,size_t n);
int zxs_odir_take(int fd,uint8_t *mode,uint16_t *width,uint8_t path[ZXS_OD_PATH_MAX]);
int zxs_odir_reply(int fd,uint8_t kind,const uint8_t *base,const uint8_t *id,const uint8_t *op);
int zxs_odir_read(int fd,const struct zxs_smb_tree_cut *parent,struct zxs_smb_tree_cut *out);
int zxs_odir_own_parent(struct zxs_smb_tree_cut *child,struct zxs_smb_tree_cut *parent);
int zxs_odir_ack(int fd,const uint8_t id[16],const uint8_t op[16]);
int zxs_odir_take_ack(int fd,const uint8_t id[16],const uint8_t op[16]);
int zxs_odir_done(int fd,const uint8_t id[16],const uint8_t op[16]);
int zxs_odir_read_done(int fd,const uint8_t id[16],const uint8_t op[16]);
int zxs_odir_rows(int fd,const uint8_t data[ZXS_DIR_PACKET-16]);
int zxs_odir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]);
#define ZXS_RETAINED_DIR 244
#define ZXS_RETAINED_OPEN 245
#define ZXS_RETAINED_SHARE_DELETE 246
#define ZXS_NS_MAXIMUM 247
#define ZXS_FILE_BIRTH 248
#define ZXS_FILE_BIRTH_LAST (ZXS_FILE_BIRTH+3)
#define ZXS_BIRTH_ACCEPT 1u
#define ZXS_BIRTH_OCCUPIED 2u
#define ZXS_BIRTH_BUSY 3u
#define ZXS_BIRTH_DENIED 4u
#define ZXS_BIRTH_FULL 5u
int zxs_ns_folder_query(int tree,const uint8_t *name,size_t n,unsigned access);
int zxs_ns_folder_read(int fd,const uint8_t pool[16],struct zxs_smb_tree_cut *);
int zxs_r_round_own_parent(struct zxs_smb_tree_cut *child,struct zxs_smb_tree_cut *parent);
int zxs_ns_dir_reply(int fd,const uint8_t data[ZXS_DIR_PACKET-16]);
int zxs_ns_dir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]);
int zxs_r_round_rows(int fd,const uint8_t data[ZXS_DIR_PACKET-16]);
int zxs_r_round_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]);
bool zxs_delete_hex(const uint8_t *name,size_t n);
int zxs_r_member_query(int tree,const uint8_t *name,size_t n,bool share_delete);
/* The FILE-open return endpoint lives with this exact original FSP. No host
 * file, database descriptor, Unix inode or disk pathname crosses to smbd. */
#define ZXS_MEMBER_OPEN 255
#define ZXS_MEMBER_WRITE_OPEN 254
#define ZXS_MEMBER_BYTES 88
struct zxs_member_info {uint8_t pool[16],volume[16],entry[16],file[16];uint64_t number,revision,size;};
int zxs_member_query(int tree,const uint8_t *name,size_t n);
int zxs_member_write_query(int tree,const uint8_t *name,size_t n);
#define ZXS_READ_HEADER "ZXS-READ/DATA-1"
#define ZXS_READ_COMMIT "ZXS-READ/DONE-1"
#define ZXS_READ_LIMIT UINT32_C(0xffff00)
/* One original member FSP, one request, one private bounded reply stream.
 * Header/data are tentative; only the exact terminal marker releases a reply. */
int zxs_read_query(int member,uint64_t offset,uint32_t count);
#define ZXS_WORK_READ 1u
#define ZXS_WORK_WRITE 2u
#define ZXS_WORK_EOF 3u
#define ZXS_WORK_FLUSH 4u
#define ZXS_WORK_DENIED 1u
#define ZXS_WORK_BUSY 2u
#define ZXS_WORK_FULL 3u
#define ZXS_WORK_MALFORMED 4u
#define ZXS_WORK_REPLY 32u
int zxs_work_query(int member,uint8_t command,uint64_t offset,uint32_t count,uint8_t settle);
int zxs_work_take(int member,int worker,uint8_t *command,uint64_t *offset,uint32_t *count,uint8_t *settle);
int zxs_work_input(int fd,uint8_t *out,size_t n);
int zxs_work_answer_live(int fd);
int zxs_work_reply(int fd,uint8_t status,uint64_t eof);
int zxs_work_result(const uint8_t data[ZXS_WORK_REPLY],uint8_t *status,uint64_t *eof);
int zxs_read_take(int member,int worker,uint64_t *offset,uint32_t *count);
int zxs_read_fill(int answer,const uint8_t *bytes,size_t n);
int zxs_read_accept(int answer);
int zxs_member_held(int fd);
int zxs_tree_events(int tree,int session,const int *members,size_t n,uint8_t *events);
int zxs_member_reply(int fd,const uint8_t data[ZXS_MEMBER_BYTES]);
int zxs_member_result(int fd,const struct zxs_smb_tree_cut *root,struct zxs_member_info *out);
int zxs_birth_query(int tree,const uint8_t *name,size_t n,unsigned flags);
int zxs_birth_reply(int fd,const uint8_t data[ZXS_MEMBER_BYTES],uint8_t kind);
int zxs_birth_result(int fd,const struct zxs_smb_tree_cut *,struct zxs_member_info *);
int zxs_birth_ack(int fd,const uint8_t file[16]);
int zxs_birth_take_ack(int fd,const uint8_t file[16]);
int zxs_birth_done(int fd,const uint8_t file[16]);
int zxs_birth_read_done(int fd,const uint8_t file[16]);
int zxs_dir_reply(int fd,const uint8_t data[ZXS_DIR_PACKET-16]);
int zxs_dir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]);
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
