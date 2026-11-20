/* SPDX-License-Identifier: GPL-3.0-or-later */
static bool zxs_create_wait_cancel(struct tevent_req *req)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	return zxs_dir_wait_cancel(req,s->smb2req->xconn->client->raw_ev_ctx,&s->zxs_wait,NULL);
}
static void zxs_dir_opened(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_member_info member;int fd=-1;bool file=s->fname[0]!=0;
	NTSTATUS status=file?zxs_member_cut_recv(sub,&member,&fd):zxs_dir_cut_recv(sub,NULL);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(tevent_req_nterror(req,status))return;
	status=zxs_open_member_fsp(s->smb2req,s->smb1req,s->zxs_access,s->zxs_share,
				   s->fname,file?&member:NULL,&fd,&s->result);
	if(fd>=0)close(fd); /* native FSP did not take ownership */
	if(!NT_STATUS_IS_OK(status) && open_was_deferred(s->smb1req->xconn,s->smb1req->mid)){
		SMBPROFILE_IOBYTES_ASYNC_SET_IDLE(s->smb2req->profile);return;
	}
	if(tevent_req_nterror(req,status))return;
	s->op=s->result->op;s->info=FILE_WAS_OPENED;
	status=smbd_smb2_create_after_exec(req);
	if(tevent_req_nterror(req,status))return;
	smbd_smb2_create_finish(req); /* terminal: callback may already have received/freed req */
}
