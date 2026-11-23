#ifndef ZXS_DESKTOP_FSKIT_ABI_H
#define ZXS_DESKTOP_FSKIT_ABI_H
#include <stddef.h>
#include <stdint.h>
/* Optional signed foreground app only, macOS 27+. No caller paths or credentials. */
int32_t zxs_fskit_release_identity(char *output,intptr_t capacity); /* Strict signed current host/module + native profile. */
int32_t zxs_fskit_group_fd(void); /* Owned fixed group FD, or negative errno. */
/* 1 starts explicit read-only preflight; 0 queries, 2 consumes, 3 cancels only the preflight. Result 0 pending,
 * 1 present/signed/enabled, negative errno refusal; zero DOES NOT certify availability. */
int32_t zxs_fskit_preflight(const char *token,int32_t verb);
int32_t zxs_fskit_session(const char *token,const char *sha256,uint64_t *out);
/* Main-thread actions 1 attach asynchronously, 2 stop opening, 3 normal detach, 4 release a never-attempted local slot,
 * 5 park after independently proven system disappearance (never a provider-clean assertion).
 * A return value of zero from 1 does NOT attest mount success. No release or auto-retry. */
int32_t zxs_fskit_action(uint64_t session,int32_t action);
int32_t zxs_fskit_status(uint64_t session,int32_t *state,int32_t *last_error,char *path,intptr_t capacity);
/* Cold completed-only evidence (known token+digest), no attach or network. */
int32_t zxs_fskit_completed(const char *token,const char *sha256,const char *scope_sha256,uint64_t *out);
/* Three caller-owned descriptors: sealed RO body, original parent, owner lease.
 * UTF-8 canonical evidence buffer is NUL-terminated on success. */
int32_t zxs_fskit_lend(uint64_t session,const char *container,int32_t fds[3],char *proof,intptr_t capacity);
int32_t zxs_fskit_release_completed(uint64_t session);
#endif
