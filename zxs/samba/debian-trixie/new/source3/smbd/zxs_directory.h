/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef ZXS_SMB_DIRECTORY_H
#define ZXS_SMB_DIRECTORY_H
#include "smbd/zxs_file_vfs.h"
#include "smbd/zxs_business.h"
struct zxs_dir_search {
	const struct smbXsrv_session *session;
	const struct smbXsrv_tcon *tcon;
	uint32_t seq;
	bool used,ended,busy;
	char mask[1025];
	uint8_t after[ZXS_DIR_NAME+1]; /* length, bytes, no on-disk or OS name */
};
static inline struct zxs_dir_search *zxs_find_dir(const struct files_struct *fsp)
{
	if (fsp==NULL || fsp->conn==NULL || !zxs_file_vfs_active(fsp->conn) ||
	    !fsp->fsp_flags.is_directory) {return NULL;}
	return vfs_fetch_fsp_extension(fsp->conn->vfs_handles,fsp);
}
struct aio_req_fsp_link;
bool zxs_dir_wait_cancel(struct tevent_req *,struct tevent_context *,
			 struct tevent_req **,struct aio_req_fsp_link **);
struct tevent_req *zxs_dir_cut_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
				    unsigned limit,const uint8_t *after,size_t size);
NTSTATUS zxs_dir_cut_recv(struct tevent_req *,uint8_t *optional_rows);
bool zxs_request_root_current(struct smbd_smb2_request *);
NTSTATUS zxs_open_dir_fsp(struct smbd_smb2_request *,struct smb_request *,uint32_t,uint32_t,
			   struct files_struct **);
NTSTATUS zxs_marshal_child(TALLOC_CTX *,struct connection_struct *,uint32_t,const char *,
			   uint64_t,uint64_t,int,char *,char **,char *,uint64_t *);
#endif
