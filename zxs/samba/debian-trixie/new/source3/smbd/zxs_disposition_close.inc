/* SPDX-License-Identifier: GPL-3.0-or-later
 * Only an explicit native CLOSE can send the marked last FILE to R.
 * No share-mode lock survives as a held kernel/DB lock across the copy. */
static bool zxs_delete_close_cancel(struct tevent_req *req)
{
	struct smbd_smb2_close_state *s=tevent_req_data(req,struct smbd_smb2_close_state);
	if(s->zxs_wait==NULL||zxs_smb2_birth_cancel_advisory(s->smb2req,true))return false;
	TALLOC_FREE(s->zxs_wait);s->zxs_unknown=true;
	tevent_req_defer_callback(req,s->smb2req->xconn->client->raw_ev_ctx);
	tevent_req_done(req);return true;
}
static void zxs_close_deleted(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_close_state *s=tevent_req_data(req,struct smbd_smb2_close_state);
	NTSTATUS status=zxs_xfer_recv(sub,&s->zxs_unknown,NULL);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	s->zxs_moved=!s->zxs_unknown&&NT_STATUS_IS_OK(status);
	zxs_finish_explicit_close(req,status);
}
static void zxs_close_ordinary(struct tevent_req *req,struct tevent_context *ev)
{
	struct smbd_smb2_close_state *s=tevent_req_data(req,struct smbd_smb2_close_state);
	struct zxs_read_fsp *m=zxs_find_read(s->in_fsp);
	uint8_t stamp[16]={0};
	int last;
	if(m==NULL||!zxs_close_same_root(s->smb2req,s->in_fsp)){
		zxs_finish_explicit_close(req,NT_STATUS_FILE_CLOSED);return;
	}
	/* New opens of this marked name get DELETE_PENDING while the
	 * selected last FSP (and only that FSP) waits for the R receipt. */
	last=zxs_claim_explicit_delete(s->in_fsp,stamp);
	if(last!=1){
		m->share_detached=last==2;
		zxs_finish_explicit_close(req,last<0?NT_STATUS_UNSUCCESSFUL:NT_STATUS_OK);return;
	}
	m->recycle_close=true;
	s->zxs_wait=zxs_xfer_send(s,ev,req,s->smb2req,s->in_fsp,ZXS_XFER_DELETE,NULL,NULL,stamp);
	if(s->zxs_wait==NULL){zxs_finish_explicit_close(req,NT_STATUS_NO_MEMORY);return;}
	tevent_req_set_cancel_fn(req,zxs_delete_close_cancel);
	tevent_req_set_callback(s->zxs_wait,zxs_close_deleted,req);
}
