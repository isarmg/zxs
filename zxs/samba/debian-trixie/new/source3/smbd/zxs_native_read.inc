/* SPDX-License-Identifier: GPL-3.0-or-later */
#include "smbd/zxs_read_delivery.inc"
static bool zxs_read_same(struct smbd_smb2_read_state *s)
{
    struct smbd_smb2_request *n=s->smb2req;files_struct *fsp=s->fsp;
    const struct zxs_read_fsp *f=zxs_find_read(fsp);
    uint64_t dev=0,ext=0;
    if(f==NULL)return false;
    for(size_t i=0;i<8;i++){dev=(dev<<8)|f->member.pool[i];ext=(ext<<8)|f->member.pool[8+i];}
    return f->fd>=0 && zxs_member_held(f->fd)==0 &&
        zxs_request_root_current(n) && f->tcon==n->tcon && f->session==n->session &&
        f->seq==n->session->global->auth_session_info_seqnum && fsp->conn==n->tcon->compat &&
        fsp->vuid==n->session->global->session_wire_id && fsp->op!=NULL && fsp->op->global!=NULL &&
        fsp->fsp_flags.is_fsa && !fsp->fsp_flags.is_pathref && !fsp->fsp_flags.closing &&
        fsp->fsp_flags.can_read && fsp->fsp_flags.can_lock && (fsp->access_mask&SEC_FILE_READ_DATA)!=0 &&
        fsp->file_id.devid==dev && fsp->file_id.extid==ext && fsp->file_id.inode==f->member.number && memcmp(f->member.pool,n->tcon->zxs_tree_choice->pool,16)==0 &&
        memcmp(f->member.volume,n->tcon->zxs_tree_choice->volume,16)==0 &&
        change_to_user_and_service(fsp->conn,fsp->vuid);
}
static bool zxs_read_brl(struct smbd_smb2_read_state *s)
{
    files_struct *fsp=s->fsp;struct lock_struct lock;
    if(fsp->conn->params==NULL || !lp_locking(fsp->conn->params)||lp_strict_locking(fsp->conn->params)!=true||lp_posix_locking(fsp->conn->params))return false;
    init_strict_lock_struct(fsp,fsp->op->global->open_persistent_id,s->in_offset,s->in_length,
            READ_LOCK,lp_posix_cifsu_locktype(fsp),&lock);
    return zxs_strict_lock_check(fsp,&lock); /* original Samba lock state; no disk VFS */
}
static bool zxs_read_cancel(struct tevent_req *req)
{
    struct smbd_smb2_read_state *s=tevent_req_data(req,struct smbd_smb2_read_state);
    return zxs_dir_wait_cancel(req,s->smb2req->xconn->client->raw_ev_ctx,&s->zxs_wait,&s->zxs_link);
}
static void zxs_read_finished(struct tevent_req *sub)
{
    struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
    struct smbd_smb2_read_state *s=tevent_req_data(req,struct smbd_smb2_read_state);
    uint8_t *bytes=NULL;size_t size=0;
    NTSTATUS status=zxs_delivery_recv(sub,s,&bytes,&size);
    if(NT_STATUS_IS_OK(status)){
        if(!zxs_read_same(s)){status=NT_STATUS_ACCESS_DENIED;}
        else if(!zxs_read_brl(s)){status=NT_STATUS_FILE_LOCK_CONFLICT;}
        else{
            s->out_data.data=talloc_move(s,&bytes);
            status=smb2_read_complete(req,(ssize_t)size,0);
        }
    }
    TALLOC_FREE(bytes);s->zxs_wait=NULL;TALLOC_FREE(sub);TALLOC_FREE(s->zxs_link);
    if(tevent_req_nterror(req,status))return;
    tevent_req_done(req);
}
static bool zxs_read_begin(struct tevent_req *req,struct tevent_context *ev)
{
    struct smbd_smb2_read_state *s=tevent_req_data(req,struct smbd_smb2_read_state);
    const struct zxs_read_fsp *f;uint64_t remaining;
    if(!zxs_read_same(s)){tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return false;}
    if(!vfs_valid_pread_range((off_t)s->in_offset,(size_t)s->in_length)){
        tevent_req_nterror(req,NT_STATUS_INVALID_PARAMETER);return false;
    }
    if(!zxs_read_brl(s)){tevent_req_nterror(req,NT_STATUS_FILE_LOCK_CONFLICT);return false;}
    f=zxs_find_read(s->fsp);remaining=s->in_offset<f->member.size?f->member.size-s->in_offset:0;
    s->zxs_link=aio_add_req_to_fsp(s->fsp,req);
    if(tevent_req_nomem(s->zxs_link,req))return false;
    talloc_steal(s,s->zxs_link);
    s->zxs_wait=zxs_delivery_send(s,ev,f->fd,s->in_offset,s->in_length,MIN(remaining,(uint64_t)s->in_length));
    if(s->zxs_wait==NULL){TALLOC_FREE(s->zxs_link);tevent_req_nterror(req,NT_STATUS_NO_MEMORY);return false;}
    smb2_request_set_async_internal(s->smb2req,true);
    tevent_req_set_cancel_fn(req,zxs_read_cancel);
    tevent_req_set_callback(s->zxs_wait,zxs_read_finished,req);return true;
}
