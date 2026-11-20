/* SPDX-License-Identifier: GPL-3.0-or-later
 * Compiled for selftest only. No arguments, files, captured messages, socket,
 * real accounts or output. Exercise Samba's own client/server implementations
 * on a fixed artificial password. This is NOT a Windows integration test. */
#include "includes.h"
#include "libcli/auth/libcli_auth.h"
#include "libcli/auth/ntlm_check.h"
#include <gnutls/gnutls.h>

int main(void)
{
	static const char login[]="fixture.smb", domain[]="ZXSLOCAL";
	static const char artificial[]="Public source-only QA sample, never deploy";
	uint8_t seed[8]={1,2,3,4,5,6,7,8},end_av[4]={0};
	DATA_BLOB challenge={.data=seed,.length=sizeof(seed)},names={.data=end_av,.length=sizeof(end_av)};
	DATA_BLOB response=data_blob_null,expected=data_blob_null,nt=data_blob_null,lm=data_blob_null;
	DATA_BLOB no_lm=data_blob_null;
	struct samr_Password stored={0};
	TALLOC_CTX *frame=NULL;
	int rc=1;
	NTSTATUS status;
	if (gnutls_global_init()<0) return 1;
	if (gnutls_fips140_mode_enabled()!=0) {gnutls_global_deinit();return 77;}
	frame=talloc_new(NULL);
	if (frame==NULL || !E_md4hash(artificial,stored.hash) ||
	    !SMBNTLMv2encrypt(frame,login,domain,artificial,&challenge,&names,NULL,&response,NULL,&expected) ||
	    expected.data==NULL || expected.length!=16 || response.data==NULL || response.length<48) goto done;
	status=ntlm_password_check(frame,false,NTLM_AUTH_NTLMV2_ONLY,0,&challenge,&no_lm,&response,
				  login,login,domain,NULL,&stored,&nt,&lm);
	if (!NT_STATUS_IS_OK(status) || nt.length!=16 || lm.length!=0 ||
	    !data_blob_equal_const_time(&nt,&expected)) goto done;
	data_blob_clear_free(&nt);
	status=ntlm_password_check(frame,false,NTLM_AUTH_NTLMV2_ONLY,0,&challenge,&no_lm,&response,
				  login,login,"",NULL,&stored,&nt,&lm);
	if (NT_STATUS_IS_OK(status) || nt.length!=0 || lm.length!=0) goto done;
	response.data[0]^=1;
	status=ntlm_password_check(frame,false,NTLM_AUTH_NTLMV2_ONLY,0,&challenge,&no_lm,&response,
				  login,login,domain,NULL,&stored,&nt,&lm);
	if (NT_STATUS_IS_OK(status) || nt.length!=0 || lm.length!=0) goto done;
	rc=0;
done:
	data_blob_clear_free(&response);data_blob_clear_free(&expected);
	data_blob_clear_free(&nt);data_blob_clear_free(&lm);explicit_bzero(&stored,sizeof(stored));
	TALLOC_FREE(frame);gnutls_global_deinit();return rc;
}
