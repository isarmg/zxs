/* SPDX-License-Identifier: GPL-3.0-or-later
 * The one top TCON and its exact named-folder lease. No disk paths. */
bool zxs_request_root_current(struct smbd_smb2_request *n)
{
	struct smbXsrv_tcon *t=n==NULL?NULL:n->tcon,*found=NULL;
	if (t==NULL || !NT_STATUS_IS_OK(t->status) || t->global==NULL ||
	    t->compat==NULL || t->compat->tcon!=t || !zxs_file_vfs_active(t->compat) ||
	    t->zxs_tree_choice==NULL || t->zxs_tree_choice->root_fd<0 || t->zxs_tree_choice->mode!=0 ||
	    (!n->do_signing && !n->was_encrypted) || zxs_smb_session_channel(n)<0 ||
	    !zxs_smb_session_pool(n->session,t->zxs_tree_choice->pool)) {return false;}
	return NT_STATUS_IS_OK(smb2srv_tcon_lookup(n->session,t->global->tcon_wire_id,
		timeval_to_nttime(&n->request_time),&found)) && found==t &&
	       zxs_smb_ctl_live(t->zxs_tree_choice->root_fd)==0;
}
bool zxs_request_folder_current(struct smbd_smb2_request *n,const struct zxs_smb_tree_cut *folder)
{
	return zxs_request_root_current(n) && folder!=NULL && folder->root_fd>=0 &&
	       (folder->mode==1 || folder->mode==2) && folder->revision!=0 &&
	       (zxs_folder_retained(folder) || (folder->volume_ordinal>=2 && folder->volume_ordinal<64)) &&
	       memcmp(folder->pool,n->tcon->zxs_tree_choice->pool,16)==0 &&
	       (!(zxs_folder_round(folder)||zxs_folder_ordinary_child(folder)) || (folder->owns_parent && folder->parent_fd>=0 &&
	         zxs_smb_ctl_live(folder->parent_fd)==0)) &&
	       zxs_smb_ctl_live(folder->root_fd)==0;
}
/* Cancellation detaches the inner watcher/AIO before the native outer
 * shutdown proceeds. TDIS and LOGOFF still use that outer object. */
_PUBLIC_
bool zxs_dir_wait_cancel(struct tevent_req *req,struct tevent_context *ev,
			 struct tevent_req **wait,struct aio_req_fsp_link **link)
{
	if(*wait==NULL)return false;
	TALLOC_FREE(*wait);
	if(link!=NULL){TALLOC_FREE(*link);}
	tevent_req_defer_callback(req,ev);
	tevent_req_nterror(req,NT_STATUS_CANCELLED);
	return true;
}

struct zxs_dir_cut_state {
	struct smbd_smb2_request *native;
	struct smbXsrv_session *session;
	struct smbXsrv_tcon *tcon;
	struct tevent_req *readable;
	uint32_t seq;
	int fd;
	unsigned limit;
	bool in_folder,offered;
	uint8_t expected_round[16];
	struct zxs_smb_tree_cut folder;
	struct zxs_member_info member;
	uint8_t data[ZXS_DIR_PACKET-16];
};
static bool zxs_make_limit(unsigned limit)
{
	return limit>=ZXS_FILE_BIRTH && limit<=ZXS_FILE_BIRTH_LAST;
}
static bool zxs_member_open_limit(unsigned limit)
{
	return limit==ZXS_MEMBER_OPEN || limit==ZXS_MEMBER_WRITE_OPEN || zxs_make_limit(limit) ||
	       limit==ZXS_RETAINED_OPEN || limit==ZXS_RETAINED_SHARE_DELETE;
}
static bool zxs_folder_open_limit(unsigned limit)
{
	return limit==ZXS_NS_OPEN || limit==ZXS_NS_WRITE || limit==ZXS_NS_MAXIMUM || limit==ZXS_RETAINED_DIR || limit==ZXS_OD_PATH;
}
static int zxs_dir_cut_close(struct zxs_dir_cut_state *s)
{
	TALLOC_FREE(s->readable);
	if(s->fd>=0){close(s->fd);s->fd=-1;}
	return 0;
}
static void zxs_dir_cut_ready(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);
	int err=0,rc=-1;
	bool ready=wait_for_read_recv(sub,&err);
	s->readable=NULL;TALLOC_FREE(sub);
	if(!ready || s->native->session!=s->session || s->native->tcon!=s->tcon ||
	   !zxs_request_root_current(s->native) || s->session->global->auth_session_info_seqnum!=s->seq ||
	   (s->in_folder && !zxs_request_folder_current(s->native,&s->folder)) ||
	   !change_to_user_and_service(s->tcon->compat,s->session->global->session_wire_id))goto reject;
	if(s->limit==ZXS_OD_PATH){
		struct zxs_smb_tree_cut picked;
		rc=zxs_odir_read(s->fd,&s->folder,&picked);
		if(rc>=2){
			NTSTATUS status=rc==2?NT_STATUS_OBJECT_NAME_COLLISION:
				rc==3?NT_STATUS_SHARING_VIOLATION:rc==5?NT_STATUS_DISK_FULL:
				rc==6?NT_STATUS_OBJECT_NAME_NOT_FOUND:NT_STATUS_ACCESS_DENIED;
			zxs_dir_cut_close(s);tevent_req_nterror(req,status);return;
		}
		if(rc<0)goto reject;
		s->folder=picked;s->offered=rc==1;rc=0;
	}else if(zxs_folder_open_limit(s->limit)){
		uint8_t original_volume[16];memcpy(original_volume,s->folder.volume,16);
		rc=zxs_ns_folder_read(s->fd,s->tcon->zxs_tree_choice->pool,&s->folder);
		if(rc==0 && s->limit==ZXS_RETAINED_DIR &&
		   (!zxs_folder_round(&s->folder) || memcmp(s->folder.setting,s->expected_round,16)!=0 ||
		    memcmp(s->folder.volume,original_volume,16)!=0))rc=-1;
		if(rc==0 && s->limit!=ZXS_RETAINED_DIR && zxs_folder_round(&s->folder))rc=-1;
	}else if(zxs_make_limit(s->limit)){
		rc=zxs_birth_result(s->fd,&s->folder,&s->member);
		if(rc==ZXS_BIRTH_ACCEPT){s->offered=true;rc=0;}
		else if(rc>=ZXS_BIRTH_OCCUPIED){
			NTSTATUS status=rc==ZXS_BIRTH_OCCUPIED?NT_STATUS_OBJECT_NAME_COLLISION:
				rc==ZXS_BIRTH_BUSY?NT_STATUS_SHARING_VIOLATION:
				rc==ZXS_BIRTH_FULL?NT_STATUS_DISK_FULL:NT_STATUS_ACCESS_DENIED;
			zxs_dir_cut_close(s);tevent_req_nterror(req,status);return;
		}
	}else if(zxs_member_open_limit(s->limit)){rc=zxs_member_result(s->fd,&s->folder,&s->member);}
	else if(s->in_folder){
		if(!zxs_folder_retained(&s->folder))rc=zxs_odir_result(s->fd,s->limit,s->data);
		else if(!zxs_folder_round(&s->folder))rc=zxs_r_round_result(s->fd,s->limit,s->data);
		else rc=zxs_dir_result(s->fd,s->limit,s->data);
	}
	else {rc=zxs_ns_dir_result(s->fd,s->limit,s->data);}
	if(rc!=0)goto reject;
	if(!zxs_member_open_limit(s->limit) && !zxs_folder_open_limit(s->limit)){zxs_dir_cut_close(s);}
	tevent_req_done(req);return;
reject:
	zxs_dir_cut_close(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);
}
static struct tevent_req *zxs_dir_cut_begin(TALLOC_CTX *ctx,struct tevent_context *ev,
	struct smbd_smb2_request *n,const struct zxs_smb_tree_cut *folder,
	unsigned limit,const uint8_t *after,size_t size,uint8_t mode)
{
	struct tevent_req *req;struct zxs_dir_cut_state *s;
	req=tevent_req_create(ctx,&s,struct zxs_dir_cut_state);if(req==NULL)return NULL;
	s->fd=-1;talloc_set_destructor(s,zxs_dir_cut_close);
	if(!zxs_request_root_current(n) || (folder!=NULL && !zxs_request_folder_current(n,folder)))goto reject;
	s->native=n;s->session=n->session;s->tcon=n->tcon;s->seq=n->session->global->auth_session_info_seqnum;s->limit=limit;
	if(talloc_reference(s,s->session)==NULL || talloc_reference(s,s->tcon)==NULL)goto reject;
	s->in_folder=folder!=NULL;if(folder!=NULL)s->folder=*folder;
	int fd=folder==NULL?s->tcon->zxs_tree_choice->root_fd:folder->root_fd;
	if(limit==ZXS_OD_PATH){
		if(folder==NULL || zxs_folder_retained(folder) || zxs_folder_ordinary_child(folder))goto reject;
		s->fd=zxs_odir_query(fd,mode,after,size);
	}else if(limit==ZXS_RETAINED_DIR){
		if(!zxs_folder_retained(folder) || zxs_folder_round(folder) || !zxs_delete_hex(after,size))goto reject;
		for(size_t i=0;i<size;i++){
			uint8_t a=after[i];a=(a<='9')?a-'0':a-'a'+10;
			s->expected_round[i/2]=(uint8_t)((s->expected_round[i/2]<<4)|a);
		}
		s->fd=zxs_ns_folder_query(fd,after,size,0);
	}else if(limit==ZXS_RETAINED_OPEN || limit==ZXS_RETAINED_SHARE_DELETE){
		if(!zxs_folder_retained(folder) || zxs_folder_round(folder))goto reject;
		s->fd=zxs_r_member_query(fd,after,size,limit==ZXS_RETAINED_SHARE_DELETE);
	}else if(zxs_folder_open_limit(limit)){
		if(folder!=NULL)goto reject;
		s->fd=zxs_ns_folder_query(fd,after,size,limit==ZXS_NS_OPEN?0:(limit==ZXS_NS_WRITE?1:2));
	}else if(zxs_make_limit(limit)){
		if(folder==NULL || zxs_folder_retained(folder))goto reject;
		s->fd=zxs_birth_query(fd,after,size,limit-ZXS_FILE_BIRTH);
	}else if(zxs_member_open_limit(limit)){
		if(folder==NULL || zxs_folder_retained(folder) || (limit==ZXS_MEMBER_WRITE_OPEN&&folder->mode!=2))goto reject;
		s->fd=limit==ZXS_MEMBER_WRITE_OPEN?zxs_member_write_query(fd,after,size):zxs_member_query(fd,after,size);
	}else{
		s->fd=zxs_dir_query(fd,limit,after,size);
	}
	if(s->fd<0)goto reject;
	s->readable=wait_for_read_send(s,ev,s->fd,false);
	if(s->readable==NULL || !tevent_req_set_endtime(s->readable,ev,timeval_current_ofs(10,0)))goto reject;
	tevent_req_set_callback(s->readable,zxs_dir_cut_ready,req);return req;
reject:
	zxs_dir_cut_close(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return tevent_req_post(req,ev);
}
struct tevent_req *zxs_dir_cut_send(TALLOC_CTX *ctx,struct tevent_context *ev,
	struct smbd_smb2_request *n,const struct zxs_smb_tree_cut *folder,unsigned limit,const uint8_t *after,size_t size)
{
	if(limit==ZXS_OD_PATH)return NULL;
	return zxs_dir_cut_begin(ctx,ev,n,folder,limit,after,size,0);
}
struct tevent_req *zxs_odir_cut_send(TALLOC_CTX *ctx,struct tevent_context *ev,struct smbd_smb2_request *n,
	const struct zxs_smb_tree_cut *folder,uint8_t mode,const uint8_t *path,size_t size)
{
	return zxs_dir_cut_begin(ctx,ev,n,folder,ZXS_OD_PATH,path,size,mode);
}
NTSTATUS zxs_dir_cut_recv(struct tevent_req *req,uint8_t *optional_rows)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);
	NTSTATUS status;
	if(tevent_req_is_nterror(req,&status)){return status;}
	if(s->limit>ZXS_DIR_ROWS)return NT_STATUS_ACCESS_DENIED;
	if(optional_rows!=NULL)memcpy(optional_rows,s->data,sizeof(s->data));
	return NT_STATUS_OK;
}

NTSTATUS zxs_folder_cut_recv(struct tevent_req *req,struct zxs_smb_tree_cut *out)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);NTSTATUS status;
	if(tevent_req_is_nterror(req,&status))return status;
	if(!zxs_folder_open_limit(s->limit) || s->fd<0 || zxs_smb_ctl_live(s->fd)!=0)return NT_STATUS_ACCESS_DENIED;
	*out=s->folder;out->root_fd=s->fd;s->fd=-1;return NT_STATUS_OK;
}
NTSTATUS zxs_odir_cut_recv(struct tevent_req *req,struct zxs_smb_tree_cut *out,bool *offered)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);
	if(s->limit!=ZXS_OD_PATH)return NT_STATUS_ACCESS_DENIED;
	NTSTATUS status=zxs_folder_cut_recv(req,out);
	if(NT_STATUS_IS_OK(status))*offered=s->offered;
	return status;
}
NTSTATUS zxs_member_cut_recv(struct tevent_req *req,struct zxs_member_info *out,int *fd,bool *offer)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);NTSTATUS status;
	*fd=-1;
	if(tevent_req_is_nterror(req,&status))return status;
	if(!zxs_member_open_limit(s->limit) || s->fd<0 || zxs_smb_ctl_live(s->fd)!=0)return NT_STATUS_ACCESS_DENIED;
	*out=s->member;*offer=s->offered;*fd=s->fd;s->fd=-1;return NT_STATUS_OK;
}
