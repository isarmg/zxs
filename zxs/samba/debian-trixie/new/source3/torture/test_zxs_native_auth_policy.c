/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source fixture: synthetic public sentinels only. No daemon, password, NT
 * response, key generation, network connection or attempt to authenticate. */
#include "includes.h"
#include "lib/util/debug.h"
#include "librpc/gen_ndr/ndr_ntlmssp.h"
#include "auth/ntlmssp/zxs_server_policy.h"

struct capture { bool safe, leak; };
static void receive(void *priv, int level, const char *msg)
{
	struct capture *c=priv;
	if (strstr(msg,"ZXS_SAFE_OUTCOME") != NULL) { c->safe = true; }
	if (level > 0 || strstr(msg,"ZXSMARK") != NULL ||
	    strstr(msg,"5A 58 53") != NULL || strstr(msg,"5a 58 53") != NULL) {
		c->leak = true;
	}
}
/* Synthetic type fixture only: an all-zero placeholder, public version
 * bytes, a reserved value and AV end, not a client proof or auth packet.
 * Use the exact upstream decoder called by ntlmssp_server_preauth. */
static bool wire_type_fits(TALLOC_CTX *ctx, uint8_t current, uint8_t maximum,
			   size_t supplied)
{
	uint8_t public_shape[48] = {0};
	DATA_BLOB raw={.data=public_shape, .length=supplied};
	struct NTLMv2_RESPONSE parsed={0};
	enum ndr_err_code err;
	if (supplied > sizeof(public_shape)) { return false; }
	public_shape[16]=current; public_shape[17]=maximum;
	public_shape[18]=7;
	err=ndr_pull_struct_blob(&raw,ctx,&parsed,
				(ndr_pull_flags_fn_t)ndr_pull_NTLMv2_RESPONSE);
	return NDR_ERR_CODE_IS_SUCCESS(err) && zxs_native_v2_type(&parsed);
}
int main(void)
{
	struct capture c={0};
	TALLOC_CTX *tmp = talloc_new(NULL);
	struct NTLMv2_RESPONSE shape={0};
	uint8_t public_bytes[16]="ZXSMARK";
	DATA_BLOB nt = {.length=sizeof(public_bytes), .data=public_bytes};
	DATA_BLOB empty = data_blob_null;
	if (tmp==NULL || !wire_type_fits(tmp,1,1,48) ||
	    wire_type_fits(tmp,2,1,48) || wire_type_fits(tmp,1,0,48) ||
	    wire_type_fits(tmp,1,1,31)) { return 1; }
	TALLOC_FREE(tmp);
	shape.Challenge.RespType=1; shape.Challenge.HiRespType=1;
	shape.Challenge.Reserved1=7; /* reserved on receipt, not an identity */
	if (!zxs_native_v2_type(&shape) || !zxs_native_v2_base_key(&nt,&empty)) { return 1; }
	shape.Challenge.HiRespType=0;
	if (zxs_native_v2_type(&shape) || zxs_native_v2_base_key(&nt,&nt)) { return 1; }
	shape.Challenge.HiRespType=1; shape.Challenge.RespType=2; nt.length--;
	if (zxs_native_v2_type(&shape) || zxs_native_v2_base_key(&nt,&empty)) { return 1; }
	nt.length=16; nt.data=NULL;
	if (zxs_native_v2_base_key(&nt,&empty)) { return 1; }

	debug_set_callback(&c,receive);
	if (!debug_parse_levels("10 auth:10 smb2:10")) { return 1; }
	debuglevel_set_class(DBGC_ALL,99);
	debuglevel_set_class(DBGC_AUTH,99);
	if (DEBUGLEVEL > 0 || debuglevel_get_class(DBGC_AUTH) > 0 ||
	    debuglevel_get_class(DBGC_SMB2) > 0) { return 1; }
	/* Private build must not print even the original SMB2 bad-signature
	 * diagnostic; these are public fixture bytes, not MACs or keys. */
	dump_data(0,public_bytes,sizeof(public_bytes));
	dump_data_dbgc(DBGC_SMB2,0,public_bytes,sizeof(public_bytes));
	dump_data_skip_zeros(0,public_bytes,sizeof(public_bytes));
	dump_data_diff(DBGC_SMB2,0,false,public_bytes,sizeof(public_bytes),
		       (const uint8_t *)"not a signature",15);
	dump_data(1,public_bytes,sizeof(public_bytes)); /* outer SPNEGO framing */
	dump_data(2,public_bytes,sizeof(public_bytes)); /* native NTLM parser */
	dump_data_pw("ZXSMARK",public_bytes,sizeof(public_bytes));
	DEBUGC(DBGC_AUTH,10,("ZXSMARK\n"));
	DEBUG(0,("ZXS_SAFE_OUTCOME\n"));
	dbgflush();
	return c.safe && !c.leak ? 0 : 1;
}
