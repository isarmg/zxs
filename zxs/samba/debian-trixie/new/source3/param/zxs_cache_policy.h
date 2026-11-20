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

/* Admit only bounded and independently sized contexts. A server without
 * caching or durable opens ignores the initial RqLs/DHnQ hints; reconnect,
 * replay and all other contexts are a different capability. QFid alone
 * selects a fixed response for the exact original FSP. */
static inline bool zxs_smb_uncached_open(uint8_t level,DATA_BLOB context,bool *qfid)
{
	const uint8_t *p=context.data;
	size_t n=context.length;
	unsigned seen=0;
	*qfid=false;
	if(level!=SMB2_OPLOCK_LEVEL_NONE && level!=SMB2_OPLOCK_LEVEL_II &&
	   level!=SMB2_OPLOCK_LEVEL_EXCLUSIVE && level!=SMB2_OPLOCK_LEVEL_BATCH &&
	   level!=SMB2_OPLOCK_LEVEL_LEASE)return false;
	if(n==0)return true;
	if(p==NULL)return false;
	while(n!=0){
		size_t next,span,len,off,last=20;
		unsigned bit;
		if(n<20)return false;
		next=IVAL(p,0);span=next==0?n:next;
		if((next!=0 && (next<24 || (next&7)!=0 || next>n-20)) ||
		   SVAL(p,4)!=16 || SVAL(p,6)!=4)return false;
		len=IVAL(p,12);
		if(memcmp(p+16,"QFid",4)==0 && len==0){bit=1;}
		else if(memcmp(p+16,"RqLs",4)==0 && level==SMB2_OPLOCK_LEVEL_LEASE &&
			(len==0x20 || len==0x34)){bit=2;}
		else if(memcmp(p+16,"DHnQ",4)==0 && level==SMB2_OPLOCK_LEVEL_BATCH && len==16){bit=4;}
		else{return false;}
		if((seen&bit)!=0)return false;
		seen|=bit;
		/* Reserved bytes and DataOffset for an empty body are ignored. */
		if(len!=0){
			off=SVAL(p,10);
			if(off<24 || (off&7)!=0 || off>span || len>span-off)return false;
			last=off+len;
		}
		if(span-last>7)return false;
		p+=span;n-=span;
	}
	*qfid=(seen&1)!=0;
	return true;
}
#define ZXS_QFID_REPLY_BYTES 56u
/* The single logical volume exposed by this server instance, not a host
 * FS identifier. MS-FSCC's all-ones unavailable file number is preserved. */
static inline bool zxs_qfid_response(DATA_BLOB body,uint64_t number)
{
	if(body.data==NULL || body.length!=ZXS_QFID_REPLY_BYTES || number==0)return false;
	memset(body.data,0,body.length);
	SSVAL(body.data,4,16);SSVAL(body.data,6,4);SSVAL(body.data,10,24);SIVAL(body.data,12,32);
	memcpy(body.data+16,"QFid",4);
	SBVAL(body.data,24,number);SBVAL(body.data,32,1);
	return true;
}
#endif
