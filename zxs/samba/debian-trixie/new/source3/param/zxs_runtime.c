/* SPDX-License-Identifier: GPL-3.0-or-later
 * A protected, explicit host installation supplies /instance. Never infer
 * its provenance from a SID, two filenames, the daemon uid or these bytes.
 * Debian 13 Linux/amd64; not a probe of the system Samba installation. */
#include "includes.h"
#include "param/zxs_runtime.h"
#include "lib/util/tini.h"
#include "libcli/security/security.h"
#include <gnutls/crypto.h>
#include <fcntl.h>
#include <linux/magic.h>
#include <linux/stat.h>
#include <sys/statfs.h>
#include <sys/statvfs.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>

#define ZXS_CFG_LIMIT (1024U * 1024U)
#define ZXS_BOUND_BYTES 128
#define ZXS_LIVE "/instance/smb.conf"

enum { ZXS_FD_ROOT, ZXS_FD_BOUND, ZXS_FD_CFG, ZXS_FD_PRIV, ZXS_FD_STATE, ZXS_FD_COUNT };
static const char *const names[ZXS_FD_COUNT] = {
	"/instance", "domain.bound", "smb.conf", "private", "state"
};
struct selected {
	int fd[ZXS_FD_COUNT];
	struct statx original[ZXS_FD_COUNT];
	struct statx store;
	uint8_t binding[ZXS_BOUND_BYTES];
};
/* The supervisor starts this private smbd with one host selection. Its
 * children inherit the original descriptors; reads use pread, not an OFD
 * seek position. A replacement must not refresh this process's identity. */
static struct selected held;
static bool active;

static bool observed(int fd, const char *name, unsigned flags, struct statx *o)
{
	const unsigned int need = STATX_TYPE | STATX_MODE | STATX_UID | STATX_GID |
		STATX_INO | STATX_MNT_ID | STATX_NLINK | STATX_SIZE | STATX_CTIME | STATX_MTIME;
	return syscall(SYS_statx, fd, name, flags | AT_NO_AUTOMOUNT, need, o) == 0 &&
		(o->stx_mask & need) == need && o->stx_ino != 0;
}
static bool entity(const struct statx *a, const struct statx *b)
{
	return a->stx_ino == b->stx_ino && a->stx_dev_major == b->stx_dev_major &&
		a->stx_dev_minor == b->stx_dev_minor;
}
static bool identical(const struct statx *a, const struct statx *b, bool file)
{
	if (!entity(a,b) || a->stx_mnt_id != b->stx_mnt_id || a->stx_mode != b->stx_mode ||
	    a->stx_uid != b->stx_uid || a->stx_gid != b->stx_gid) {
		return false;
	}
	/* Private/state directories may legitimately gain or lose ordinary
	 * Samba state. That is not a change of their host-selected identity. */
	return !file || (a->stx_size == b->stx_size && a->stx_nlink == b->stx_nlink &&
		a->stx_mtime.tv_sec == b->stx_mtime.tv_sec && a->stx_mtime.tv_nsec == b->stx_mtime.tv_nsec &&
		a->stx_ctime.tv_sec == b->stx_ctime.tv_sec && a->stx_ctime.tv_nsec == b->stx_ctime.tv_nsec);
}
static bool readonly_file(int fd)
{
	struct statvfs s;
	return fstatvfs(fd,&s) == 0 && (s.f_flag & ST_RDONLY) != 0;
}
static bool still_here(const struct selected *x)
{
	int i;
	for (i=0;i<ZXS_FD_COUNT;i++) {
		struct statx fd, name;
		bool file = (i==ZXS_FD_BOUND || i==ZXS_FD_CFG);
		int parent = i==ZXS_FD_ROOT ? AT_FDCWD : x->fd[ZXS_FD_ROOT];
		if (!observed(x->fd[i],"",AT_EMPTY_PATH,&fd) ||
		    !observed(parent,names[i],AT_SYMLINK_NOFOLLOW,&name) ||
		    !identical(&x->original[i],&fd,file) || !identical(&fd,&name,file) ||
		    (file && !readonly_file(x->fd[i]))) {
			return false;
		}
	}
	/* Public config/BOUND and the two named directories can be checked
	 * without searching the protected private directory as a client. */
	return true;
}
/* Pin before opening for data, excluding symlinks, devices and FIFOs. The
 * procfs magic link reopens that very pin, not a second user-controlled name. */
static int readable_pin(int pin, const struct statx *orig)
{
	struct statfs s;
	struct statx got;
	char label[32];
	int n, fd = -1, proc = open("/proc/self/fd",O_RDONLY|O_DIRECTORY|O_CLOEXEC);
	if (proc == -1) { return -1; }
	n = snprintf(label,sizeof(label),"%d",pin);
	if (n > 0 && (size_t)n < sizeof(label) && fstatfs(proc,&s)==0 &&
	    (unsigned long)s.f_type == PROC_SUPER_MAGIC) {
		fd = openat(proc,label,O_RDONLY|O_CLOEXEC|O_NONBLOCK);
	}
	close(proc);
	if (fd != -1 && (!observed(fd,"",AT_EMPTY_PATH,&got) ||
	    !identical(orig,&got,true) || !readonly_file(fd))) {
		close(fd); fd=-1;
	}
	return fd;
}
static bool exact_bytes(int fd, uint8_t *dst, size_t size)
{
	size_t at=0;
	while (at<size) {
		ssize_t n=pread(fd,dst+at,size-at,(off_t)at);
		if (n<0 && errno==EINTR) { continue; }
		if (n<=0) { return false; }
		at+=(size_t)n;
	}
	return true;
}
static bool nonzero(const uint8_t *bytes, size_t n)
{
	size_t i;
	for (i=0;i<n;i++) { if (bytes[i]!=0) { return true; } }
	return false;
}
static bool valid_record(const uint8_t *b)
{
	static const uint8_t prefix[12] = {1,4,0,0,0,0,0,5,21,0,0,0};
	return memcmp(b,"ZXS-SAM/BOUND-1",16)==0 && nonzero(b+16,16) &&
		nonzero(b+32,16) && nonzero(b+48,16) && memcmp(b+96,"ZXSLOCAL",8)==0 &&
		memcmp(b+104,prefix,sizeof(prefix))==0 && nonzero(b+116,12);
}
static bool select_original(void)
{
	int i;
	struct selected x;
	if (active) { return still_here(&held); }
	ZERO_STRUCT(x);
	for (i=0;i<ZXS_FD_COUNT;i++) { x.fd[i]=-1; }
	for (i=0;i<ZXS_FD_COUNT;i++) {
		bool file=(i==ZXS_FD_BOUND || i==ZXS_FD_CFG);
		int p=(i==ZXS_FD_ROOT ? AT_FDCWD : x.fd[ZXS_FD_ROOT]);
		int flags=O_PATH|O_NOFOLLOW|O_CLOEXEC|(file ? 0 : O_DIRECTORY);
		struct statx *st=&x.original[i];
		x.fd[i]=openat(p,names[i],flags);
		if (x.fd[i]<0 || !observed(x.fd[i],"",AT_EMPTY_PATH,st) ||
		    (st->stx_mode&0022)!=0 || (file ? !S_ISREG(st->stx_mode) : !S_ISDIR(st->stx_mode))) {
			goto fail;
		}
		if (file) {
			int pin=x.fd[i];
			if ((i==ZXS_FD_BOUND && st->stx_size!=ZXS_BOUND_BYTES) ||
			    (i==ZXS_FD_CFG && (st->stx_size==0 || st->stx_size>ZXS_CFG_LIMIT))) { goto fail; }
			x.fd[i]=readable_pin(pin,st); close(pin);
			if (x.fd[i]<0) { goto fail; }
		}
	}
	if (!observed(x.fd[ZXS_FD_PRIV],"secrets.tdb",AT_SYMLINK_NOFOLLOW,&x.store) ||
	    !S_ISREG(x.store.stx_mode) || (x.store.stx_mode&07777)!=0600 ||
	    x.store.stx_nlink!=1 || x.store.stx_size==0 ||
	    x.store.stx_mnt_id!=x.original[ZXS_FD_PRIV].stx_mnt_id ||
	    entity(&x.original[ZXS_FD_PRIV],&x.original[ZXS_FD_STATE]) ||
	    entity(&x.original[ZXS_FD_PRIV],&x.original[ZXS_FD_ROOT]) || entity(&x.original[ZXS_FD_STATE],&x.original[ZXS_FD_ROOT]) ||
	    entity(&x.original[ZXS_FD_BOUND],&x.original[ZXS_FD_CFG]) ||
	    !exact_bytes(x.fd[ZXS_FD_BOUND],x.binding,sizeof(x.binding)) || !valid_record(x.binding) ||
	    !still_here(&x)) { goto fail; }
	held=x; active=true; return true;
fail:
	for (i=0;i<ZXS_FD_COUNT;i++) { if (x.fd[i]>=0) { close(x.fd[i]); } }
	return false;
}
static bool record_intact(void)
{
	uint8_t now[ZXS_BOUND_BYTES];
	return active && still_here(&held) && exact_bytes(held.fd[ZXS_FD_BOUND],now,sizeof(now)) &&
		memcmp(held.binding,now,sizeof(now))==0;
}
/* Digest and parse the very same bytes, not two sequential pathname opens.
 * Calls the existing Samba parser with exactly its normal callback order. */
bool zxs_runtime_config(const char *name, bool (*section)(const char *, void *),
			bool (*param)(const char *, const char *, void *), void *ctx)
{
	uint8_t digest[32], *text=NULL;
	FILE *stream=NULL;
	size_t n;
	bool ok=false;
	if (name==NULL || strcmp(name,ZXS_LIVE)!=0 || !select_original() || !record_intact()) { return false; }
	n=(size_t)held.original[ZXS_FD_CFG].stx_size;
	text=malloc(n);
	if (text==NULL || !exact_bytes(held.fd[ZXS_FD_CFG],text,n) || memchr(text,0,n)!=NULL ||
	    gnutls_hash_fast(GNUTLS_DIG_SHA256,text,n,digest)!=0 ||
	    memcmp(digest,held.binding+64,32)!=0 || !still_here(&held)) { goto out; }
	stream=fmemopen(text,n,"r");
	if (stream==NULL) { goto out; }
	ok=tini_parse(stream,false,section,param,ctx) && !ferror(stream);
	if (fclose(stream)!=0) { ok=false; }
	ok=ok && record_intact();
out:
	free(text); return ok;
}
bool zxs_runtime_store(dev_t dev, ino_t ino)
{
	struct statx named;
	/* Native dbwrap has already opened the exact TDB. Metadata only: never
	 * open/close the database in parallel with its native lock holder. */
	return record_intact() &&
		observed(held.fd[ZXS_FD_PRIV],"secrets.tdb",AT_SYMLINK_NOFOLLOW,&named) &&
		identical(&held.store,&named,true) && held.store.stx_ino==ino &&
		held.store.stx_dev_major==(unsigned)major(dev) &&
		held.store.stx_dev_minor==(unsigned)minor(dev);
}
bool zxs_runtime_domain(const struct dom_sid *s)
{
	const uint8_t *b=held.binding+104;
	int i;
	if (s==NULL || !record_intact() || s->sid_rev_num!=b[0] ||
	    s->num_auths!=b[1] || memcmp(s->id_auth,b+2,6)!=0) { return false; }
	for (i=0;i<4;i++) {
		if (s->sub_auths[i]!=IVAL(b,8+4*i)) { return false; }
	}
	return true;
}
