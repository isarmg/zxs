/* SPDX-License-Identifier: GPL-3.0-or-later
 * Native QUERY per existing protocol FSP. after is a byte-order cursor, not
 * an authorization. Do not commit it until this complete reply is ready. */
static bool zxs_search_same(struct smbd_smb2_query_directory_state *s)
{
	struct zxs_dir_search *d=zxs_find_dir(s->dirfsp);
	struct smbd_smb2_request *n=s->smb2req;
	return d!=NULL && d==s->zxs_search && zxs_request_root_current(n) && s->dirfsp->conn==n->tcon->compat &&
		s->dirfsp->vuid==n->session->global->session_wire_id &&
		d->tcon==n->tcon && d->session==n->session && d->seq==n->session->global->auth_session_info_seqnum &&
		(d->folder.mode==0 || zxs_request_folder_current(n,&d->folder)) &&
		(s->dirfsp->access_mask&SEC_DIR_LIST)!=0 &&
		change_to_user_and_service(s->dirfsp->conn,s->dirfsp->vuid);
}
static void zxs_search_release(struct smbd_smb2_query_directory_state *s)
{
	TALLOC_FREE(s->zxs_cut);
	TALLOC_FREE(s->zxs_link);
	if(s->zxs_busy){s->zxs_search->busy=false;s->zxs_busy=false;}
}
static int zxs_search_destroy(struct smbd_smb2_query_directory_state *s)
{
	zxs_search_release(s);return 0;
}
static bool zxs_search_cancel(struct tevent_req *req)
{
	struct smbd_smb2_query_directory_state *s=tevent_req_data(req,struct smbd_smb2_query_directory_state);
	if(!s->zxs_busy || s->zxs_cut==NULL)return false;
	s->zxs_busy=false;s->zxs_search->busy=false;
	return zxs_dir_wait_cancel(req,s->smb2req->xconn->client->raw_ev_ctx,&s->zxs_cut,&s->zxs_link);
}
static void zxs_search_commit(struct tevent_req *req,bool ended)
{
	struct smbd_smb2_query_directory_state *s=tevent_req_data(req,struct smbd_smb2_query_directory_state);
	struct zxs_dir_search *d=s->zxs_search;
	d->used=true;d->ended=ended;
	memcpy(d->after,s->zxs_after,sizeof(d->after));
	memcpy(d->mask,s->zxs_mask,sizeof(d->mask));
	zxs_search_release(s);
	if(s->num==0){tevent_req_nterror(req,s->empty_status);return;}
	SIVAL(s->out_output_buffer.data,s->last_entry_off,0);
	tevent_req_done(req);
}
static uint64_t zxs_row_u64(const uint8_t *p)
{
	uint64_t n=0;for(size_t i=0;i<8;i++)n=(n<<8)|p[i];return n;
}
static void zxs_search_schedule(struct tevent_req *req);
static void zxs_search_received(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_query_directory_state *s=tevent_req_data(req,struct smbd_smb2_query_directory_state);
	uint8_t rows[ZXS_DIR_PACKET-16];unsigned i;
	NTSTATUS status=zxs_dir_cut_recv(sub,rows);
	s->zxs_cut=NULL;TALLOC_FREE(sub);
	if(!NT_STATUS_IS_OK(status) || !zxs_search_same(s)){
		zxs_search_release(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	if(s->zxs_eof){zxs_search_commit(req,true);return;}
	for(i=0;i<rows[0];i++){
		const uint8_t *r=rows+1+i*ZXS_DIR_SLOT;
		char name[ZXS_DIR_NAME+1];
		const size_t len=r[0], prior=s->zxs_after[0];
		int order=memcmp(r+1,s->zxs_after+1,MIN(prior,len));
		uint64_t off=0;
		if(order<0 || (order==0 && len<=prior)){
			zxs_search_release(s);tevent_req_nterror(req,NT_STATUS_INVALID_NETWORK_RESPONSE);return;
		}
		memcpy(name,r+1,len);name[len]=0;
		if(mask_match(name,s->zxs_mask,s->dirfsp->conn->case_sensitive)){
			const struct zxs_smb_tree_cut *f=&s->zxs_search->folder;
			const bool ordinary=f->mode!=0&&!zxs_folder_retained(f);
			const bool tagged_dir=ordinary&&(r[1+ZXS_DIR_NAME+8]&0x80)!=0;
			const uint8_t row_kind=f->mode==0?ZXS_ROW_TOP:
				(zxs_folder_retained(f)&&!zxs_folder_round(f))?ZXS_ROW_ROUND:
				tagged_dir?ZXS_ROW_ORDINARY_DIR:ZXS_ROW_FILE;
			status=zxs_marshal_child(s,s->dirfsp->conn,s->info_level,name,row_kind,
				zxs_row_u64(r+1+ZXS_DIR_NAME),tagged_dir?0:zxs_row_u64(r+1+ZXS_DIR_NAME+8),
				s->in_output_buffer_length-(int)s->out_output_buffer.length,
				s->base_data,&s->pdata,s->end_data,&off);
			if(NT_STATUS_EQUAL(status,STATUS_MORE_ENTRIES)){
				if(s->num!=0){zxs_search_commit(req,false);return;}
				zxs_search_release(s);tevent_req_nterror(req,NT_STATUS_INFO_LENGTH_MISMATCH);return;
			}
			if(!NT_STATUS_IS_OK(status)){zxs_search_release(s);tevent_req_nterror(req,status);return;}
			s->last_entry_off=(int)off;s->num++;
			s->out_output_buffer.length=s->pdata-s->base_data;
		}
		s->zxs_after[0]=(uint8_t)len;memcpy(s->zxs_after+1,r+1,len);
		if(s->num>=s->max_count){i++;break;}
	}
	if(s->num!=0 || rows[0]<ZXS_DIR_ROWS){
		zxs_search_commit(req,i==rows[0] && rows[0]<ZXS_DIR_ROWS);return;
	}
	zxs_search_schedule(req); /* whole page failed the original Samba mask */
}
static void zxs_search_schedule(struct tevent_req *req)
{
	struct smbd_smb2_query_directory_state *s=tevent_req_data(req,struct smbd_smb2_query_directory_state);
	if(++s->zxs_pages>512){zxs_search_release(s);tevent_req_nterror(req,NT_STATUS_RETRY);return;}
	const struct zxs_smb_tree_cut *folder=s->zxs_search->folder.mode==0?NULL:&s->zxs_search->folder;
	s->zxs_cut=zxs_dir_cut_send(s,s->ev,s->smb2req,folder,s->zxs_eof?0:ZXS_DIR_ROWS,
		s->zxs_eof?NULL:s->zxs_after+1,s->zxs_eof?0:s->zxs_after[0]);
	if(s->zxs_cut==NULL){zxs_search_release(s);tevent_req_nterror(req,NT_STATUS_NO_MEMORY);return;}
	tevent_req_set_callback(s->zxs_cut,zxs_search_received,req);
}
static void zxs_search_begin(struct tevent_req *req,uint8_t flags,uint32_t idx)
{
	struct smbd_smb2_query_directory_state *s=tevent_req_data(req,struct smbd_smb2_query_directory_state);
	struct zxs_dir_search *d=zxs_find_dir(s->dirfsp);
	const bool restart=(flags&(SMB2_CONTINUE_FLAG_RESTART|SMB2_CONTINUE_FLAG_REOPEN))!=0;
	size_t n=strlen(s->in_file_name);
	size_t capacity;
	s->zxs_search=d;
	if(!zxs_search_same(s)){tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;}
	if(idx!=0 || (flags&~(SMB2_CONTINUE_FLAG_RESTART|SMB2_CONTINUE_FLAG_REOPEN|SMB2_CONTINUE_FLAG_SINGLE)) ||
	   s->info_level==FSCC_FILE_POSIX_INFORMATION || n>=sizeof(s->zxs_mask) ||
	   (!d->used && n==0) || (d->used && !restart && n && strcmp(d->mask,s->in_file_name)!=0)){
		tevent_req_nterror(req,NT_STATUS_INVALID_PARAMETER);return;
	}
	if(d->busy){tevent_req_nterror(req,NT_STATUS_REQUEST_NOT_ACCEPTED);return;}
	s->empty_status=(!d->used || restart)?NT_STATUS_NO_SUCH_FILE:STATUS_NO_MORE_FILES;
	if(n){memcpy(s->zxs_mask,s->in_file_name,n+1);}else{memcpy(s->zxs_mask,d->mask,sizeof(d->mask));}
	if(!restart){memcpy(s->zxs_after,d->after,sizeof(d->after));s->zxs_eof=d->ended;}
	s->max_count=(flags&SMB2_CONTINUE_FLAG_SINGLE)?1:ZXS_DIR_ROWS;
	/* A real page has at most 16 entries, each <=222 UTF-8 bytes; 16 KiB
	 * exceeds their largest UTF-16 ID_BOTH encoding, even for ASCII. */
	s->in_output_buffer_length=MIN(s->in_output_buffer_length,16384);
	/* Compound output header allocation can fail. Reserve it on the
	 * outer native request before advancing any per-FSP search state. */
	s->zxs_reply_header=smbd_smb2_generate_outbody(s->smb2req,8);
	if(tevent_req_nomem(s->zxs_reply_header.data,req))return;
	capacity=(size_t)s->in_output_buffer_length+4096;
	s->out_output_buffer=data_blob_talloc(s,NULL,capacity);
	if(tevent_req_nomem(s->out_output_buffer.data,req))return;
	s->out_output_buffer.length=0;s->base_data=(char *)s->out_output_buffer.data;
	s->pdata=s->base_data;s->end_data=s->base_data+capacity-1;
	s->zxs_link=aio_add_req_to_fsp(s->dirfsp,req);
	if(s->zxs_link==NULL){tevent_req_nterror(req,NT_STATUS_NO_MEMORY);return;}
	talloc_steal(s,s->zxs_link); /* one link across all physical pages and waits */
	s->zxs_busy=true;d->busy=true;
	talloc_set_destructor(s,zxs_search_destroy);tevent_req_set_cancel_fn(req,zxs_search_cancel);
	smb2_request_set_async_internal(s->smb2req,true);zxs_search_schedule(req);
}
