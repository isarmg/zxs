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

#define ZXS_CREATE_QFID_QUERY 1u
#define ZXS_CREATE_MXAC_QUERY 2u
#define ZXS_CREATE_REPLY_CONTEXTS (ZXS_CREATE_QFID_QUERY|ZXS_CREATE_MXAC_QUERY)

/* Admit only bounded and independently sized contexts. AAPL's initial
 * server query detects other SMB servers by the absence of an AAPL reply;
 * it does not request file capabilities. A server without caching or durable opens ignores the initial RqLs/DHnQ hints; reconnect,
 * replay and all other contexts are a different capability. QFid/MxAc
 * select fixed responses for the exact original FSP. */
static inline bool zxs_smb_uncached_open(uint8_t level,DATA_BLOB context,uint8_t *queries)
{
	const uint8_t *p=context.data;
	size_t n=context.length;
	const unsigned aapl=16;
	unsigned seen=0;
	*queries=0;
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
		if(memcmp(p+16,"QFid",4)==0 && len==0){bit=ZXS_CREATE_QFID_QUERY;}
		else if(memcmp(p+16,"MxAc",4)==0 && (len==0 || len==8)){bit=ZXS_CREATE_MXAC_QUERY;}
		else if(memcmp(p+16,"RqLs",4)==0 && level==SMB2_OPLOCK_LEVEL_LEASE &&
			(len==0x20 || len==0x34)){bit=4;}
		else if(memcmp(p+16,"DHnQ",4)==0 && level==SMB2_OPLOCK_LEVEL_BATCH && len==16){bit=8;}
		else if(memcmp(p+16,"AAPL",4)==0 && len==24){bit=aapl;}
		else{return false;}
		if((seen&bit)!=0)return false;
		seen|=bit;
		/* Reserved bytes and DataOffset for an empty body are ignored. */
		if(len!=0){
			off=SVAL(p,10);
			if(off<24 || (off&7)!=0 || off>span || len>span-off)return false;
			/* Only the initial Apple server query: four bytes each for
			 * command and reserved, then two ignored u64 bitmaps. */
			if(bit==aapl && (IVAL(p,off)!=1 || IVAL(p,off+4)!=0))return false;
			last=off+len;
		}
		if(span-last>7)return false;
		p+=span;n-=span;
	}
	*queries=seen&ZXS_CREATE_REPLY_CONTEXTS;
	return true;
}
#define ZXS_QFID_REPLY_BYTES 56u
#define ZXS_MXAC_REPLY_BYTES 32u
static inline size_t zxs_create_context_size(uint8_t queries)
{
	return ((queries&ZXS_CREATE_QFID_QUERY)?ZXS_QFID_REPLY_BYTES:0)+
	       ((queries&ZXS_CREATE_MXAC_QUERY)?ZXS_MXAC_REPLY_BYTES:0);
}
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
/* This virtual store does not provide ChangeTime. An optional timestamp
 * is never evidence that this result can be recycled. The caller now
 * supplies the current authorized open mask independently of the requested
 * access of this handle. No timestamp alters the open or the returned mask. */
static inline bool zxs_create_context_response(DATA_BLOB out,uint8_t queries,
					       uint64_t number,uint32_t allowed)
{
	if((queries&~ZXS_CREATE_REPLY_CONTEXTS)!=0 ||
	   out.length!=zxs_create_context_size(queries) ||
	   (out.length!=0 && out.data==NULL))return false;
	if(queries&ZXS_CREATE_QFID_QUERY){
		DATA_BLOB one={.data=out.data,.length=ZXS_QFID_REPLY_BYTES};
		if(!zxs_qfid_response(one,number))return false;
		if(queries&ZXS_CREATE_MXAC_QUERY)SIVAL(one.data,0,ZXS_QFID_REPLY_BYTES);
		out.data+=one.length;out.length-=one.length;
	}
	if(queries&ZXS_CREATE_MXAC_QUERY){
		memset(out.data,0,ZXS_MXAC_REPLY_BYTES);
		SSVAL(out.data,4,16);SSVAL(out.data,6,4);SSVAL(out.data,10,24);SIVAL(out.data,12,8);
		memcpy(out.data+16,"MxAc",4);
		SIVAL(out.data,24,0); /* STATUS_SUCCESS; allowed comes from the original open policy. */
		SIVAL(out.data,28,allowed);
	}
	return true;
}
#endif
