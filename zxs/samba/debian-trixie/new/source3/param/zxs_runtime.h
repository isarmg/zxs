/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private to the pinned, dedicated smbd. Neither operation authenticates a client. */
#ifndef ZXS_SAMBA_RUNTIME_H
#define ZXS_SAMBA_RUNTIME_H
#include "param/zxs_client_role.h"
#include "param/zxs_smb_control.h"
#include <pwd.h>
#include <string.h>
/* Called after an original network identity has already been checked.
 * This one host principal supplies execution identity, not authentication. */
static inline bool zxs_fixed_unix_mapping(const struct passwd *pw,const char *name,
                                          uid_t uid,gid_t gid)
{
    return pw!=NULL && pw->pw_name!=NULL && name!=NULL &&
           strcmp(pw->pw_name,"zxs-smb-client")==0 && strcmp(name,pw->pw_name)==0 &&
           uid!=0 && gid!=0 && uid!=(uid_t)-1 && gid!=(gid_t)-1 &&
           uid==pw->pw_uid && gid==pw->pw_gid;
}
struct dom_sid;
bool zxs_runtime_control_take(void);
bool zxs_runtime_connect_storage(void);
bool zxs_runtime_storage_live(void);
int zxs_runtime_watch_fd(void);
bool zxs_runtime_prepare_gate(int fd,struct zxs_client_offer *offer,int gate[2]);
void zxs_runtime_dispatch_child(const struct zxs_client_offer *offer,pid_t child,int gate[2]);
void zxs_runtime_wait_gate_or_exit(int gate);
bool zxs_runtime_claim_child(const struct zxs_client_offer *offer,int fd);
bool zxs_runtime_child_on(int fd);
bool zxs_runtime_client_process(void);
int zxs_runtime_attempt(int transport,int64_t *started_msec);
bool zxs_runtime_config(const char *name, bool (*section)(const char *, void *),
			bool (*param)(const char *, const char *, void *), void *ctx);
bool zxs_runtime_store(dev_t dev, ino_t ino);
bool zxs_runtime_domain(const struct dom_sid *actual);
#endif
