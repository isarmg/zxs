/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef ZXS_SMB_DIRECTORY_H
#define ZXS_SMB_DIRECTORY_H
#include "smbd/zxs_file_vfs.h"
#include "smbd/zxs_business.h"
static inline bool zxs_folder_retained(const struct zxs_smb_tree_cut *f)
{
	return f!=NULL && f->volume_ordinal==1 && f->mode==1 && f->revision==1;
}
static inline bool zxs_folder_round(const struct zxs_smb_tree_cut *f)
{
	const uint8_t zero[16]={0};
	return zxs_folder_retained(f) && memcmp(f->setting,zero,16)!=0;
}
static inline bool zxs_folder_ordinary_child(const struct zxs_smb_tree_cut *f)
{
	const uint8_t zero[16]={0};
	return f!=NULL && f->volume_ordinal>=2 && f->volume_ordinal<64 &&
		memcmp(f->directory,zero,16)!=0 && memcmp(f->directory_op,zero,16)!=0;
}
/* Stable complete native lock key; a directory's wire number is a separate projection. */
static inline struct file_id zxs_directory_file_id(const struct zxs_smb_tree_cut *f)
{
	struct file_id n={.inode=(uint64_t)f->volume_ordinal+1};
	const uint8_t *original=NULL;
	for(size_t i=0;i<8;i++){n.devid=(n.devid<<8)|f->pool[i];n.extid=(n.extid<<8)|f->pool[i+8];}
	if(zxs_folder_ordinary_child(f)){
		n.devid^=UINT64_C(0x5a58534f52444449);original=f->directory;
	}else if(zxs_folder_round(f)){
		n.devid^=UINT64_C(0x5a5853524f554e44);original=f->setting;
	}
	if(original!=NULL){
		n.inode=0;n.extid=0;
		for(size_t i=0;i<8;i++){n.inode=(n.inode<<8)|original[i+8];n.extid=(n.extid<<8)|original[i];}
	}
	return n;
}
static inline bool zxs_directory_same_id(const struct files_struct *fsp,const struct zxs_smb_tree_cut *f)
{
	struct file_id n=zxs_directory_file_id(f);
	const uint64_t number=(zxs_folder_ordinary_child(f)||zxs_folder_round(f))?UINT64_MAX:(uint64_t)f->volume_ordinal+1;
	return fsp->file_id.devid==n.devid && fsp->file_id.extid==n.extid && fsp->file_id.inode==n.inode &&
		fsp->fsp_name!=NULL && fsp->fsp_name->st.st_ex_ino==number;
}
struct zxs_dir_search {
	const struct smbXsrv_session *session;
	const struct smbXsrv_tcon *tcon;
	uint32_t seq;
	bool used,ended,busy;
	struct zxs_smb_tree_cut folder;
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
	uint32_t seq;int fd;
	bool writable,busy,dirty,share_detached,recycle_close;
	struct zxs_member_info member;
	struct zxs_smb_tree_cut folder;
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
				    const struct zxs_smb_tree_cut *,unsigned limit,const uint8_t *after,size_t size);
NTSTATUS zxs_dir_cut_recv(struct tevent_req *,uint8_t *optional_rows);
struct tevent_req *zxs_odir_cut_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
	const struct zxs_smb_tree_cut *,uint8_t mode,const uint8_t *path,size_t size);
NTSTATUS zxs_odir_cut_recv(struct tevent_req *,struct zxs_smb_tree_cut *,bool *offered);
bool zxs_request_root_current(struct smbd_smb2_request *);
bool zxs_smb2_birth_cancel_advisory(const struct smbd_smb2_request *,bool acknowledged);
void zxs_smb2_retire_birth(struct smbd_smb2_request *);
bool zxs_request_folder_current(struct smbd_smb2_request *,const struct zxs_smb_tree_cut *);
NTSTATUS zxs_folder_cut_recv(struct tevent_req *,struct zxs_smb_tree_cut *);
bool zxs_root_info_class(uint8_t);
bool zxs_root_info_needs_attributes(uint8_t);
NTSTATUS zxs_format_root_info(TALLOC_CTX *,struct files_struct *,uint8_t,uint32_t,DATA_BLOB *,NTSTATUS *);
NTSTATUS zxs_open_access(connection_struct *,bool directory,const struct zxs_smb_tree_cut *,uint32_t *desired,uint32_t share);
struct tevent_req *zxs_work_send(TALLOC_CTX *,struct tevent_context *,struct tevent_req *outer,
	struct smbd_smb2_request *,struct files_struct *,uint8_t,uint64_t,DATA_BLOB,bool settle,bool closing);
NTSTATUS zxs_work_recv(struct tevent_req *);
struct tevent_req *zxs_xfer_send(TALLOC_CTX *,struct tevent_context *,struct tevent_req *outer,
	struct smbd_smb2_request *,struct files_struct *,uint8_t,const char *,const char *,const uint8_t mark[16]);
NTSTATUS zxs_xfer_recv(struct tevent_req *,bool *unknown,uint8_t receipt[16]);
bool zxs_restore_input(TALLOC_CTX *,DATA_BLOB,char **,char **);
NTSTATUS zxs_open_member_fsp(struct smbd_smb2_request *,struct smb_request *,uint32_t,uint32_t,
			    const char *name,const struct zxs_member_info *,struct zxs_smb_tree_cut *,bool writable,int *fd,struct files_struct **);
NTSTATUS zxs_member_cut_recv(struct tevent_req *,struct zxs_member_info *,int *fd,bool *offer);
#define ZXS_ROW_FILE 0u
#define ZXS_ROW_TOP 1u
#define ZXS_ROW_ROUND 2u
#define ZXS_ROW_ORDINARY_DIR 3u
NTSTATUS zxs_marshal_child(TALLOC_CTX *,struct connection_struct *,uint32_t,const char *,
			   uint8_t,uint64_t,uint64_t,int,char *,char **,char *,uint64_t *);
#endif
