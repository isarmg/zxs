/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private to the pinned, dedicated smbd. Neither operation authenticates a client. */
#ifndef ZXS_SAMBA_RUNTIME_H
#define ZXS_SAMBA_RUNTIME_H
#include "param/zxs_client_role.h"
struct dom_sid;
bool zxs_runtime_control_take(void);
bool zxs_runtime_connect_storage(void);
bool zxs_runtime_storage_live(void);
int zxs_runtime_watch_fd(void);
bool zxs_runtime_offer_child(int fd,struct zxs_client_offer *offer);
bool zxs_runtime_claim_child(const struct zxs_client_offer *offer,int fd);
bool zxs_runtime_child_on(int fd);
bool zxs_runtime_client_process(void);
int zxs_runtime_attempt(int transport,int64_t *started_msec);
bool zxs_runtime_config(const char *name, bool (*section)(const char *, void *),
			bool (*param)(const char *, const char *, void *), void *ctx);
bool zxs_runtime_store(dev_t dev, ino_t ino);
bool zxs_runtime_domain(const struct dom_sid *actual);
#endif
