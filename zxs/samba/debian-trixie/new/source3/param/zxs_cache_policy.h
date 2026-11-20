/* SPDX-License-Identifier: GPL-3.0-or-later
 * This managed service provides neither file nor directory cache grants. */
#ifndef ZXS_SMB_CACHE_POLICY_H
#define ZXS_SMB_CACHE_POLICY_H
#include "libcli/smb/smb2_constants.h"
#include "lib/util/data_blob.h"

static inline uint32_t zxs_smb_uncached_caps(uint32_t caps)
{
	return caps & ~(SMB2_CAP_LEASING|SMB2_CAP_DIRECTORY_LEASING);
}

/* MS-SMB2 3.3.5.9: a non-leasing server ignores RqLs. Accept only the
 * actual one wire context, not a string prefix of a different tag, and
 * only its two standard data lengths. There is no resulting cache state.
 * Other contexts (including the separately specified MxAc) stay closed. */
static inline bool zxs_smb_uncached_open(uint8_t level,DATA_BLOB context)
{
	const uint8_t *p=context.data;
	size_t n=context.length,off,len;
	if(level!=SMB2_OPLOCK_LEVEL_NONE && level!=SMB2_OPLOCK_LEVEL_II &&
	   level!=SMB2_OPLOCK_LEVEL_EXCLUSIVE && level!=SMB2_OPLOCK_LEVEL_BATCH &&
	   level!=SMB2_OPLOCK_LEVEL_LEASE)return false;
	if(n==0)return true;
	if(level!=SMB2_OPLOCK_LEVEL_LEASE || p==NULL || n<24 || IVAL(p,0)!=0 ||
	   SVAL(p,4)!=16 || SVAL(p,6)!=4 || memcmp(p+16,"RqLs",4)!=0)return false;
	off=SVAL(p,10);len=IVAL(p,12);
	return off>=24 && (off&7)==0 && off<=n && (len==0x20 || len==0x34) &&
	       len<=n-off && n-off-len<=7;
}
#endif
