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
struct zxs_read_fsp {
	const struct smbXsrv_session *session;const struct smbXsrv_tcon *tcon;
	uint32_t seq;int fd;struct zxs_member_info member;
};
static inline struct zxs_read_fsp *zxs_find_read(const struct files_struct *fsp)
{
	if (fsp==NULL || fsp->conn==NULL || !zxs_file_vfs_active(fsp->conn) || fsp->fsp_flags.is_directory)return NULL;
	return vfs_fetch_fsp_extension(fsp->conn->vfs_handles,fsp);
}
struct aio_req_fsp_link;
bool zxs_dir_wait_cancel(struct tevent_req *,struct tevent_context *,
			 struct tevent_req **,struct aio_req_fsp_link **);
struct tevent_req *zxs_dir_cut_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
				    unsigned limit,const uint8_t *after,size_t size);
NTSTATUS zxs_dir_cut_recv(struct tevent_req *,uint8_t *optional_rows);
bool zxs_request_root_current(struct smbd_smb2_request *);
bool zxs_root_info_class(uint8_t);
bool zxs_root_info_needs_attributes(uint8_t);
NTSTATUS zxs_format_root_info(TALLOC_CTX *,struct files_struct *,uint8_t,uint32_t,DATA_BLOB *,NTSTATUS *);
NTSTATUS zxs_open_access(connection_struct *,bool directory,uint32_t *desired,uint32_t share);
NTSTATUS zxs_open_member_fsp(struct smbd_smb2_request *,struct smb_request *,uint32_t,uint32_t,
			    const char *name,const struct zxs_member_info *,int *fd,struct files_struct **);
NTSTATUS zxs_member_cut_recv(struct tevent_req *,struct zxs_member_info *,int *fd);
NTSTATUS zxs_marshal_child(TALLOC_CTX *,struct connection_struct *,uint32_t,const char *,
			   uint64_t,uint64_t,int,char *,char **,char *,uint64_t *);
#endif
