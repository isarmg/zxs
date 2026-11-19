/* SPDX-License-Identifier: GPL-3.0-or-later
 * For the one trusted Samba TCP worker, before it creates any attempt.
 * A pidfd identifies a sender; it cannot make receiving FD_CLOFORK.
 * This is not a general sandbox for executing untrusted native code. */
#ifndef ZXS_SMB_RECEIVER_H
#define ZXS_SMB_RECEIVER_H
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/sched.h>
#include <linux/seccomp.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

static inline bool zxs_seal_receiver(void)
{
#if !defined(__x86_64__) || defined(__ILP32__)
	return false;
#else
	/* The live service is specifically Debian 13 amd64. A second PID
	 * cannot inherit either the private socket or the native GENSEC heap.
	 * Real threads have one TGID and share memory and descriptor table.
	 * clone3 has pointer arguments that classic seccomp cannot inspect;
	 * ENOSYS lets an ordinary pthread implementation use checked clone. */
	const uint32_t thread = CLONE_THREAD | CLONE_VM | CLONE_FILES;
	const unsigned deny = SECCOMP_RET_ERRNO | EPERM;
	struct sock_filter code[] = {
		BPF_STMT(BPF_LD|BPF_W|BPF_ABS,offsetof(struct seccomp_data,arch)),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,AUDIT_ARCH_X86_64,1,0),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_STMT(BPF_LD|BPF_W|BPF_ABS,offsetof(struct seccomp_data,nr)),
		BPF_JUMP(BPF_JMP|BPF_JSET|BPF_K,0x40000000U,0,1),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_fork,0,1),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_vfork,0,1),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_execve,0,1),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_execveat,0,1),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_clone3,0,1),
		BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ERRNO | ENOSYS),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,SYS_clone,0,4),
		BPF_STMT(BPF_LD|BPF_W|BPF_ABS,offsetof(struct seccomp_data,args[0])),
		BPF_STMT(BPF_ALU|BPF_AND|BPF_K,thread),
		BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,thread,1,0),
		BPF_STMT(BPF_RET|BPF_K,deny),
		BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ALLOW)
	};
	struct sock_fprog prog = {
		.len = sizeof(code)/sizeof(code[0]), .filter = code
	};
	if (prctl(PR_SET_NO_NEW_PRIVS,1UL,0UL,0UL,0UL)!=0) { return false; }
	/* A positive TID is a failed synchronous installation, not success. */
	return syscall(SYS_seccomp,SECCOMP_SET_MODE_FILTER,SECCOMP_FILTER_FLAG_TSYNC,&prog)==0;
#endif
}
#endif
