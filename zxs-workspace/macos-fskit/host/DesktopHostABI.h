#ifndef ZXS_DESKTOP_FSKIT_ABI_H
#define ZXS_DESKTOP_FSKIT_ABI_H
#include <stddef.h>
#include <stdint.h>
/* Optional signed foreground app only, macOS 27+. No caller paths or credentials. */
int32_t zxs_fskit_group_fd(void); /* Owned fixed group FD, or negative errno. */
/* 1 starts explicit read-only preflight; 0 queries, 2 consumes, 3 cancels only the preflight. Result 0 pending,
 * 1 present/signed/enabled, negative errno refusal; zero DOES NOT certify availability. */
int32_t zxs_fskit_preflight(const char *token,int32_t verb);
int32_t zxs_fskit_session(const char *token,const char *sha256,uint64_t *out);
/* Main-thread actions 1 attach asynchronously, 2 stop opening, 3 normal detach.
 * A return value of zero from 1 does NOT attest mount success. No release or auto-retry. */
int32_t zxs_fskit_action(uint64_t session,int32_t action);
int32_t zxs_fskit_status(uint64_t session,int32_t *state,int32_t *last_error,char *path,intptr_t capacity);
#endif
