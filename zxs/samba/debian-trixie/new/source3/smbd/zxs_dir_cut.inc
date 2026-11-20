/* SPDX-License-Identifier: GPL-3.0-or-later
 * Same accepted session, same native tcon, on its own original private pair.
 * Each query has a distinct reply endpoint; no tevent handler waits for a disk. */
bool zxs_request_root_current(struct smbd_smb2_request *n)
{
	struct smbXsrv_tcon *t=n==NULL?NULL:n->tcon,*found=NULL;
	if (t==NULL || !NT_STATUS_IS_OK(t->status) || t->global==NULL ||
	    t->compat==NULL || t->compat->tcon!=t || !zxs_file_vfs_active(t->compat) ||
	    t->zxs_tree_choice==NULL || t->zxs_tree_choice->root_fd<0 ||
	    (!n->do_signing && !n->was_encrypted) || zxs_smb_session_channel(n)<0 ||
	    !zxs_smb_session_pool(n->session,t->zxs_tree_choice->pool)) {return false;}
	return NT_STATUS_IS_OK(smb2srv_tcon_lookup(n->session,t->global->tcon_wire_id,
		timeval_to_nttime(&n->request_time),&found)) && found==t &&
	       zxs_smb_ctl_live(t->zxs_tree_choice->root_fd)==0;
}
/* TDIS/LOGOFF still walk the outer SMB request after tevent_req_cancel.
 * CLOSE expects a successful cancellation to remove its AIO link before
 * it inspects the FSP again, without dispatching another event. */
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
	struct zxs_member_info member;
	uint8_t data[ZXS_DIR_PACKET-16];
};
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
	int err=0;
	bool ready=wait_for_read_recv(sub,&err);
	s->readable=NULL;TALLOC_FREE(sub);
	if(!ready || s->native->session!=s->session || s->native->tcon!=s->tcon ||
	   !zxs_request_root_current(s->native) || s->session->global->auth_session_info_seqnum!=s->seq ||
	   (s->limit==ZXS_MEMBER_OPEN ?
	    zxs_member_result(s->fd,s->tcon->zxs_tree_choice,&s->member) : zxs_dir_result(s->fd,s->limit,s->data))!=0 ||
	   !change_to_user_and_service(s->tcon->compat,s->session->global->session_wire_id)){
		zxs_dir_cut_close(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	if(s->limit!=ZXS_MEMBER_OPEN){zxs_dir_cut_close(s);}
	tevent_req_done(req);
}
struct tevent_req *zxs_dir_cut_send(TALLOC_CTX *ctx,struct tevent_context *ev,
	struct smbd_smb2_request *n,unsigned limit,const uint8_t *after,size_t size)
{
	struct tevent_req *req;struct zxs_dir_cut_state *s;
	req=tevent_req_create(ctx,&s,struct zxs_dir_cut_state);if(req==NULL)return NULL;
	s->fd=-1;talloc_set_destructor(s,zxs_dir_cut_close);
	if(!zxs_request_root_current(n))goto reject;
	s->native=n;s->session=n->session;s->tcon=n->tcon;s->seq=n->session->global->auth_session_info_seqnum;s->limit=limit;
	if(talloc_reference(s,s->session)==NULL || talloc_reference(s,s->tcon)==NULL)goto reject;
	s->fd=limit==ZXS_MEMBER_OPEN ? zxs_member_query(s->tcon->zxs_tree_choice->root_fd,after,size) :
		zxs_dir_query(s->tcon->zxs_tree_choice->root_fd,limit,after,size);
	if(s->fd<0)goto reject;
	s->readable=wait_for_read_send(s,ev,s->fd,false);
	if(s->readable==NULL || !tevent_req_set_endtime(s->readable,ev,timeval_current_ofs(10,0)))goto reject;
	tevent_req_set_callback(s->readable,zxs_dir_cut_ready,req);return req;
reject:
	zxs_dir_cut_close(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return tevent_req_post(req,ev);
}
NTSTATUS zxs_dir_cut_recv(struct tevent_req *req,uint8_t *optional_rows)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);
	NTSTATUS status;
	if(tevent_req_is_nterror(req,&status)){return status;}
	if(optional_rows!=NULL)memcpy(optional_rows,s->data,sizeof(s->data));
	return NT_STATUS_OK;
}

NTSTATUS zxs_member_cut_recv(struct tevent_req *req,struct zxs_member_info *out,int *fd)
{
	struct zxs_dir_cut_state *s=tevent_req_data(req,struct zxs_dir_cut_state);NTSTATUS status;
	*fd=-1;
	if(tevent_req_is_nterror(req,&status))return status;
	if(s->limit!=ZXS_MEMBER_OPEN || s->fd<0 || zxs_smb_ctl_live(s->fd)!=0)return NT_STATUS_ACCESS_DENIED;
	*out=s->member;*fd=s->fd;s->fd=-1;return NT_STATUS_OK;
}
