/* SPDX-License-Identifier: GPL-3.0-or-later
 * Capacity upkeep for the managed private, nonclustered Windows BRL table.
 * Use only while its original FileId share-mode lock is held. */
#pragma once
#include "locking.h"
#include <errno.h>
#include <limits.h>
#include <signal.h>

#define ZXS_BRL_MAX_PER_FILE 4096U

static inline unsigned int zxs_brl_compact_gone(struct lock_struct *locks,
					       unsigned int count)
{
	struct { uint64_t pid; bool gone; } checked[32] = {0};
	unsigned int kept = 0;

	for (unsigned int i=0; i<count; i++) {
		const struct server_id *id = &locks[i].context.pid;

		if (locks[i].lock_flav == WINDOWS_LOCK && id->vnn == NONCLUSTER_VNN) {
			if (id->pid == 0) {
				/* Upstream already marked this entry for cleanup. */
				continue;
			}
			if (id->pid <= INT_MAX) {
				size_t j = id->pid % ARRAY_SIZE(checked);
				if (checked[j].pid != id->pid) {
					checked[j].pid = id->pid;
					/* This lock table belongs to the smbd family in one
					 * private PID namespace. EPERM, PID reuse and a
					 * missing messaging record are not proof of death. */
					checked[j].gone = kill((pid_t)id->pid, 0) == -1 &&
							  errno == ESRCH;
				}
				if (checked[j].gone) {
					continue;
				}
			}
		}
		if (kept != i) {
			locks[kept] = locks[i];
		}
		kept++;
	}
	return kept;
}
