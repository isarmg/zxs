/* SPDX-License-Identifier: Apache-2.0 */
#ifndef HELPER_NATIVE_ACCOUNT_SNAPSHOT_H
#define HELPER_NATIVE_ACCOUNT_SNAPSHOT_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum helper_native_match {
    HELPER_NATIVE_UNCONFIRMED = 0,
    HELPER_NATIVE_PREVIOUS = 1,
    HELPER_NATIVE_TARGET = 2
};

struct helper_native_expected {
    size_t login_len;
    uint8_t login[64];
    uint8_t target[154];
    bool has_previous;
    uint8_t previous[154];
};

/*
 * One private caller only: the signed offline worker inside the limited
 * instance view. The supervisor has selected the original accountdb and its
 * file, both are projected read-only, and it owns both continuous EX leases.
 * It has already verified the actual local SAM domain using separate witness
 * material. These file descriptors are O_PATHs on the projection; the worker
 * never opens a host fallback. A result is not permission, durability, or
 * proof that a supplied password belongs to the account. No secret output.
 */
enum helper_native_match helper_native_observe(int parent_pin, int store_pin,
    const struct helper_native_expected *expected);

#endif
