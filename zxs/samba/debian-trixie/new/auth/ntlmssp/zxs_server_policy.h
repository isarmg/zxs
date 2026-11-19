/* SPDX-License-Identifier: GPL-3.0-or-later
 * Only structural checks on Samba's own already parsed information. Neither
 * this header nor its test can verify a password or issue a session. */
#ifndef ZXS_NTLMSSP_SERVER_POLICY_H
#define ZXS_NTLMSSP_SERVER_POLICY_H
#include "lib/util/data_blob.h"
#include "librpc/gen_ndr/ntlmssp.h"
static inline bool zxs_native_v2_type(const struct NTLMv2_RESPONSE *r)
{
	return r != NULL && r->Challenge.RespType == 1 && r->Challenge.HiRespType == 1;
}
static inline bool zxs_native_v2_base_key(const DATA_BLOB *nt, const DATA_BLOB *lm)
{
	return nt != NULL && nt->data != NULL && nt->length == 16 &&
		lm != NULL && lm->length == 0;
}
#endif
