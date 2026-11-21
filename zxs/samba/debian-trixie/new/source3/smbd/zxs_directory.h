/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef ZXS_SMB_DIRECTORY_H
#define ZXS_SMB_DIRECTORY_H
#include "smbd/zxs_file_vfs.h"
#include "smbd/zxs_business.h"
#include "libcli/smb/smb_constants.h"
#include "libcli/smb/smb2_constants.h"
/* Empty and one-component names in the signed ZXS namespace can only
 * select the one root or an enrolled top directory. For an existing-only
 * open they need no directory hint; deeper paths select a confirmed kind. */
static inline bool zxs_create_directory_shape(const char *name,uint32_t options,uint32_t disposition,bool *directory)
{
	if(name==NULL)return false;
	if(options==FILE_DIRECTORY_FILE ||
	   (options==0 && disposition==FILE_OPEN && strchr(name,'\\')==NULL)){
		*directory=true;return true;
	}
	if(name[0]==0 || (options!=0 && options!=FILE_NON_DIRECTORY_FILE))return false;
	*directory=false;return true;
}
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
/* Wire-only number for FileInternalInformation and QFid; not a lock key. */
static inline uint64_t zxs_wire_file_number(const struct files_struct *f)
{
	if(f==NULL || f->fsp_name==NULL)return 0;
	return f->fsp_name->st.st_ex_ino==UINT64_MAX?UINT64_MAX:f->file_id.inode;
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
/* An authorized, physically observed zero-row initial scan of '*' is
 * exhaustion, not a failed search for a specific name. */
static inline NTSTATUS zxs_empty_search_status(NTSTATUS otherwise,const char *mask,bool empty)
{
	return NT_STATUS_EQUAL(otherwise,NT_STATUS_NO_SUCH_FILE) && empty && strcmp(mask,"*")==0?
		STATUS_NO_MORE_FILES:otherwise;
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
	bool writable,busy,dirty,share_detached,recycle_close,moved_parent;
	uint64_t placement_sequence;
	struct zxs_member_info member;
	struct zxs_smb_tree_cut folder;
};
static inline struct zxs_read_fsp *zxs_find_read(const struct files_struct *fsp)
{
	if (fsp==NULL || fsp->conn==NULL || !zxs_file_vfs_active(fsp->conn) || fsp->fsp_flags.is_directory)return NULL;
	return vfs_fetch_fsp_extension(fsp->conn->vfs_handles,fsp);
}
/* Scope of the first explicit ordinary range-lock implementation. Use
 * the actual 16-bit SMB2 LockCount; a generated packet isn't authority. */
#define ZXS_LOCK_RANGE_COUNT 64u
static inline bool zxs_lock_width(uint16_t count,size_t extra)
{
	return count>0 && count<=ZXS_LOCK_RANGE_COUNT && (size_t)(count-1)*24<=extra;
}
static inline bool zxs_lock_immediate_flags(uint32_t flags,bool unlock)
{
	if(unlock)return flags==SMB2_LOCK_FLAG_UNLOCK;
	return flags==(SMB2_LOCK_FLAG_SHARED|SMB2_LOCK_FLAG_FAIL_IMMEDIATELY) ||
		flags==(SMB2_LOCK_FLAG_EXCLUSIVE|SMB2_LOCK_FLAG_FAIL_IMMEDIATELY);
}
struct aio_req_fsp_link;
bool zxs_dir_wait_cancel(struct tevent_req *,struct tevent_context *,
			 struct tevent_req **,struct aio_req_fsp_link **);
struct tevent_req *zxs_dir_cut_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
				    const struct zxs_smb_tree_cut *,unsigned limit,const uint8_t *after,size_t size);
NTSTATUS zxs_dir_cut_recv(struct tevent_req *,uint8_t *optional_rows);
struct tevent_req *zxs_capacity_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
	const struct zxs_smb_tree_cut *ordinary,bool may_write,int moved_member);
NTSTATUS zxs_capacity_recv(struct tevent_req *,struct zxs_ns_space *);
#define ZXS_INFO_FS_SIZE 3u
#define ZXS_INFO_FS_ATTRIBUTE 5u
#define ZXS_INFO_FS_FULL_SIZE 7u
static inline bool zxs_capacity_info_class(uint8_t cls)
{
	return cls==ZXS_INFO_FS_SIZE || cls==ZXS_INFO_FS_FULL_SIZE;
}
static inline bool zxs_capacity_may_write(const struct connection_struct *c)
{
	return !c->read_only && (c->share_access&SEC_DIR_ADD_FILE)!=0;
}
/* Standard FileFs(Size/FullSize)Information. Use a virtual 4 KiB
 * reporting unit, not an assertion about the disk's physical sector
 * size or an allocation promise for a different directory/volume. */
static inline NTSTATUS zxs_format_capacity(TALLOC_CTX *ctx,const struct zxs_ns_space *cap,
	uint8_t cls,uint32_t max,bool caller_can_write,DATA_BLOB *out)
{
	if(!zxs_capacity_info_class(cls))return NT_STATUS_NOT_SUPPORTED;
	const bool full=cls==ZXS_INFO_FS_FULL_SIZE;
	const uint32_t length=full?32:24,dimensions=full?24:16;
	if(max<length)return NT_STATUS_INFO_LENGTH_MISMATCH;
	if(cap->status==ZXS_NS_CAP_NO_MEDIA)return NT_STATUS_DEVICE_NOT_READY;
	if(cap->status==ZXS_NS_CAP_BUSY)return NT_STATUS_INSUFFICIENT_RESOURCES;
	if(cap->status!=ZXS_NS_CAP_READY)return NT_STATUS_ACCESS_DENIED;
	DATA_BLOB b=data_blob_talloc_zero(ctx,length);
	if(b.data==NULL)return NT_STATUS_NO_MEMORY;
	SBVAL(b.data,0,cap->total/UINT64_C(4096));
	SBVAL(b.data,8,caller_can_write?cap->available/UINT64_C(4096):0);
	if(full)SBVAL(b.data,16,cap->free/UINT64_C(4096));
	SIVAL(b.data,dimensions,8);
	SIVAL(b.data,dimensions+4,512);
	*out=b;return NT_STATUS_OK;
}
/* FileFsAttributeInformation for the single, logical ZXS namespace.
 * No filesystem-specific capabilities, DOS/NTFS ACLs or host labels. */
static inline NTSTATUS zxs_format_fs_attributes(TALLOC_CTX *ctx,uint32_t max,DATA_BLOB *out,NTSTATUS *result)
{
	const uint8_t name[]={'Z',0,'X',0,'S',0};
	const uint32_t fixed=12,full=fixed+sizeof(name);
	if(max<fixed)return NT_STATUS_INFO_LENGTH_MISMATCH;
	const uint32_t n=MIN(max,full);
	DATA_BLOB b=data_blob_talloc_zero(ctx,n);
	if(b.data==NULL)return NT_STATUS_NO_MEMORY;
	SIVAL(b.data,0,7); /* CASE_SENSITIVE_SEARCH, CASE_PRESERVED_NAMES, UNICODE_ON_DISK */
	SIVAL(b.data,4,ZXS_DIR_NAME); /* Namespace's longest logical directory component. */
	SIVAL(b.data,8,sizeof(name));
	memcpy(b.data+fixed,name,n-fixed);
	*out=b;*result=n<full?NT_STATUS_BUFFER_OVERFLOW:NT_STATUS_OK;
	return NT_STATUS_OK;
}
struct tevent_req *zxs_odir_cut_send(TALLOC_CTX *,struct tevent_context *,struct smbd_smb2_request *,
	const struct zxs_smb_tree_cut *,uint8_t mode,const uint8_t *path,size_t size);
NTSTATUS zxs_odir_cut_recv(struct tevent_req *,struct zxs_smb_tree_cut *,bool *offered,bool *file_kind);
bool zxs_request_root_current(struct smbd_smb2_request *);
bool zxs_smb2_birth_cancel_advisory(const struct smbd_smb2_request *,bool acknowledged);
void zxs_smb2_retire_birth(struct smbd_smb2_request *);
bool zxs_request_folder_current(struct smbd_smb2_request *,const struct zxs_smb_tree_cut *);
NTSTATUS zxs_folder_cut_recv(struct tevent_req *,struct zxs_smb_tree_cut *);
bool zxs_root_info_class(uint8_t);
bool zxs_root_info_needs_attributes(uint8_t);
bool zxs_member_info_class(uint8_t);
bool zxs_member_info_needs_attributes(uint8_t);
NTSTATUS zxs_format_root_info(TALLOC_CTX *,struct files_struct *,uint8_t,uint32_t,DATA_BLOB *,NTSTATUS *);
NTSTATUS zxs_format_member_info(TALLOC_CTX *,struct files_struct *,uint8_t,uint32_t,bool,DATA_BLOB *,NTSTATUS *);
/* Virtual allocation as reported by CREATE, QUERY_DIRECTORY and QUERY_INFO.
 * Not Linux allocated blocks, available capacity, or reserved storage. */
static inline uint64_t zxs_logical_allocation(uint64_t eof)
{
	const uint64_t max=(uint64_t)INT64_MAX & ~UINT64_C(4095);
	return eof>max?max:(eof+UINT64_C(4095)) & ~UINT64_C(4095);
}
NTSTATUS zxs_open_access(connection_struct *,bool directory,const struct zxs_smb_tree_cut *,uint32_t *desired,uint32_t share);
bool zxs_fsp_maximum_open_access(const struct files_struct *,uint32_t *allowed);
struct tevent_req *zxs_work_send(TALLOC_CTX *,struct tevent_context *,struct tevent_req *outer,
	struct smbd_smb2_request *,struct files_struct *,uint8_t,uint64_t,DATA_BLOB,bool settle,bool closing);
NTSTATUS zxs_work_recv(struct tevent_req *);
struct tevent_req *zxs_xfer_send(TALLOC_CTX *,struct tevent_context *,struct tevent_req *outer,
	struct smbd_smb2_request *,struct files_struct *,uint8_t,const char *,const char *,const uint8_t mark[16]);
NTSTATUS zxs_xfer_recv(struct tevent_req *,bool *unknown,uint8_t receipt[16]);
bool zxs_restore_input(TALLOC_CTX *,DATA_BLOB,char **,char **);
bool zxs_ordinary_rename_input(TALLOC_CTX *,DATA_BLOB,const char *,char **,char **,const char **);
uint8_t zxs_ordinary_destination_kind(const char *opened,const char *volume);
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
