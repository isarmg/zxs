/* SPDX-License-Identifier: GPL-3.0-or-later
 * Explicit Windows range locks on the ordinary original FSP; no host FD
 * lock, durable replay, deferred conflict wait, or SQLite mutation. */
#include "lib/util/sys_rw_data.h"
#include "smbd/zxs_read_delivery.inc"

static bool zxs_lock_current(const struct smbd_smb2_lock_state *s,const struct zxs_read_fsp *f)
{
	struct smbd_smb2_request *n=s->smb2req;
	struct files_struct *fsp=s->fsp;
	uint64_t dev=0,ext=0;
	if(f==NULL || f!=zxs_find_read(fsp) || f->fd<0 || f->folder.volume_ordinal<2 ||
	   f->tcon!=n->tcon || f->session!=n->session ||
	   !NT_STATUS_IS_OK(n->xconn->transport.status) ||
	   !zxs_request_folder_current(n,&f->folder) ||
	   f->seq!=n->session->global->auth_session_info_seqnum ||
	   fsp->conn!=n->tcon->compat || fsp->vuid!=n->session->global->session_wire_id ||
	   fsp->fsp_flags.closing || fsp->fsp_flags.posix_open || fsp->fsp_flags.is_pathref ||
	   !fsp->fsp_flags.is_fsa || !fsp->fsp_flags.can_lock || fsp->op==NULL || fsp->op->global==NULL ||
	   (fsp->access_mask&(SEC_FILE_READ_DATA|SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA))==0 ||
	   fsp->conn->params==NULL || !lp_locking(fsp->conn->params) ||
	   lp_strict_locking(fsp->conn->params)!=true || lp_posix_locking(fsp->conn->params) ||
	   zxs_member_held(f->fd)!=0 ||
	   memcmp(f->member.pool,n->tcon->zxs_tree_choice->pool,16)!=0 ||
	   memcmp(f->member.volume,f->folder.volume,16)!=0 ||
	   !change_to_user_and_service(fsp->conn,fsp->vuid))return false;
	for(size_t i=0;i<8;i++){dev=(dev<<8)|f->member.pool[i];ext=(ext<<8)|f->member.pool[i+8];}
	return fsp->file_id.devid==dev && fsp->file_id.extid==ext && fsp->file_id.inode==f->member.number;
}
static int zxs_lock_release(struct smbd_smb2_lock_state *s)
{
	TALLOC_FREE(s->zxs_proof);
	s->zxs_member=NULL;
	TALLOC_FREE(s->zxs_link);
	return 0;
}
static bool zxs_lock_cancel(struct tevent_req *req)
{
	struct smbd_smb2_lock_state *s=tevent_req_data(req,struct smbd_smb2_lock_state);
	if(s->zxs_proof==NULL)return false;
	const struct smbd_smb2_request *n=s->smb2req;
	const bool stopped=s->fsp->fsp_flags.closing ||
		!NT_STATUS_IS_OK(n->session->status) || !NT_STATUS_IS_OK(n->tcon->status);
	tevent_req_defer_callback(req,n->xconn->client->raw_ev_ctx);
	zxs_lock_release(s);
	tevent_req_nterror(req,stopped?NT_STATUS_RANGE_NOT_LOCKED:NT_STATUS_CANCELLED);return true;
}

struct zxs_brl_cut {struct smbd_smb2_lock_state *state;NTSTATUS status;};
static void zxs_locked_range(struct share_mode_lock *lck,struct byte_range_lock *brl,void *opaque)
{
	struct zxs_brl_cut *cut=opaque;
	(void)lck;
	if(brl==NULL){cut->status=NT_STATUS_ACCESS_DENIED;return;}
	struct smbd_do_locks_state lock={
		.num_locks=cut->state->lock_count,.locks=cut->state->locks,
	};
	if(!zxs_brl_group_room(brl,lock.num_locks)){
		cut->status=NT_STATUS_INSUFFICIENT_RESOURCES;return;
	}
	cut->status=smbd_do_locks_try(brl,&lock);
}
static NTSTATUS zxs_apply_ranges(struct smbd_smb2_lock_state *s)
{
	if(s->zxs_unlock)return smbd_do_unlocking(s->smb1req,s->fsp,s->lock_count,s->locks);
	struct zxs_brl_cut cut={.state=s,.status=NT_STATUS_INTERNAL_ERROR};
	NTSTATUS status=share_mode_do_locked_brl(s->fsp,zxs_locked_range,&cut);
	if(!NT_STATUS_IS_OK(status))return status;
	/* Do not complete a tevent request under the native lock or before
	 * its actual backing-memory range inventory has been written back. */
	if(NT_STATUS_EQUAL(cut.status,NT_STATUS_FILE_LOCK_CONFLICT))return NT_STATUS_LOCK_NOT_GRANTED;
	if(NT_STATUS_EQUAL(cut.status,NT_STATUS_RETRY))return NT_STATUS_LOCK_NOT_GRANTED;
	return cut.status;
}
static void zxs_lock_proved(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_lock_state *s=tevent_req_data(req,struct smbd_smb2_lock_state);
	size_t n=0;uint8_t *bytes=NULL;NTSTATUS status=zxs_delivery_recv(sub,s,&bytes,&n);
	s->zxs_proof=NULL;TALLOC_FREE(sub);TALLOC_FREE(bytes);
	if(NT_STATUS_IS_OK(status)){
		if(n!=0 || !zxs_lock_current(s,s->zxs_member))status=NT_STATUS_ACCESS_DENIED;
		else if(s->zxs_member->busy)status=NT_STATUS_SHARING_VIOLATION;
		else status=zxs_apply_ranges(s);
	}
	zxs_lock_release(s);
	if(tevent_req_nterror(req,status))return;
	tevent_req_done(req);
}
static bool zxs_lock_begin(struct tevent_req *req,bool unlock)
{
	struct smbd_smb2_lock_state *s=tevent_req_data(req,struct smbd_smb2_lock_state);
	struct zxs_read_fsp *f=zxs_find_read(s->fsp);
	NTSTATUS status=NT_STATUS_ACCESS_DENIED;
	if(!zxs_lock_current(s,f))goto failed;
	if(f->busy){status=NT_STATUS_SHARING_VIOLATION;goto failed;}
	s->zxs_body=smbd_smb2_generate_outbody(s->smb2req,0x04);
	if(s->zxs_body.data==NULL){status=NT_STATUS_NO_MEMORY;goto failed;}
	s->zxs_link=aio_add_req_to_fsp(s->fsp,req);
	if(s->zxs_link==NULL){status=NT_STATUS_NO_MEMORY;goto failed;}
	talloc_steal(s,s->zxs_link);
	talloc_set_destructor(s,zxs_lock_release);
	/* This read-only proof must not occupy the mutation seat or stop an
	 * otherwise compatible same-FSP read. AIO still pins the exact FSP. */
	s->zxs_member=f;s->zxs_unlock=unlock;
	s->zxs_proof=zxs_delivery_send(s,s->ev,f->fd,0,0,0);
	if(s->zxs_proof==NULL){status=NT_STATUS_NO_MEMORY;goto failed;}
	tevent_req_set_cancel_fn(req,zxs_lock_cancel);
	tevent_req_set_callback(s->zxs_proof,zxs_lock_proved,req);
	if(smbd_smb2_is_compound(s->smb2req)&&!smbd_smb2_is_last_in_compound(s->smb2req)){
		smb2_request_set_async_internal(s->smb2req,true);
	}
	return true;
failed:
	zxs_lock_release(s);tevent_req_nterror(req,status);return false;
}
