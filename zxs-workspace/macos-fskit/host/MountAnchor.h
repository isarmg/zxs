#ifndef ZXS_MOUNT_ANCHOR_H
#define ZXS_MOUNT_ANCHOR_H
#include <stdint.h>

typedef struct zxs_mount_anchor zxs_mount_anchor;
zxs_mount_anchor *zxs_mount_before(void);
int zxs_mount_bind(zxs_mount_anchor *, const char *system_path, const unsigned char expected_uuid[16],
                   uint64_t expected_root);
int zxs_mount_live(zxs_mount_anchor *); /* Original registered FSID/path/flags in current nonblocking VFS snapshot. */
int zxs_mount_check(zxs_mount_anchor *);
int zxs_mount_normal(zxs_mount_anchor *); /* 0 means original gone; EBUSY preserves normal retry. */
void zxs_mount_release(zxs_mount_anchor *); /* Does not unmount, retry, remove or submit anything. */
#endif
