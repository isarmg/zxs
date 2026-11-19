/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private to the pinned, dedicated smbd. Neither operation authenticates a client. */
#ifndef ZXS_SAMBA_RUNTIME_H
#define ZXS_SAMBA_RUNTIME_H
struct dom_sid;
bool zxs_runtime_config(const char *name, bool (*section)(const char *, void *),
			bool (*param)(const char *, const char *, void *), void *ctx);
bool zxs_runtime_store(dev_t dev, ino_t ino);
bool zxs_runtime_domain(const struct dom_sid *actual);
#endif
