/* SPDX-License-Identifier: GPL-3.0-or-later
 * Local native session lifetime; never serialize or look these up by SID. */
#ifndef ZXS_SMB_BUSINESS_H
#define ZXS_SMB_BUSINESS_H
struct smbXsrv_session;
struct smbd_smb2_request;
void zxs_smb_session_retire(struct smbXsrv_session *session);
bool zxs_smb_session_pool(const struct smbXsrv_session *session,const uint8_t pool[16]);
int zxs_smb_session_channel(struct smbd_smb2_request *request);
#endif
