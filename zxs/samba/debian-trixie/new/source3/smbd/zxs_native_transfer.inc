/* SPDX-License-Identifier: GPL-3.0-or-later
 * Same native TreeId, original FSP, private typed transfer request. No
 * path conversion calls the native filesystem or a rename/unlink VFS. */
struct zxs_xfer_state{
	struct smbd_smb2_request *native;
	struct files_struct *fsp;
	struct zxs_read_fsp *member;
	struct aio_req_fsp_link *link;
	struct tevent_req *watch;
	int answer;
	bool unknown;
	uint8_t kind,receipt[16];
};
static bool zxs_xfer_current(struct smbd_smb2_request *n,struct files_struct *fsp,
			    const struct zxs_read_fsp *m,uint8_t kind)
{
	uint64_t dev=0,ext=0;
	if(m==NULL||m!=zxs_find_read(fsp)||m->fd<0||!zxs_request_folder_current(n,&m->folder)||
	   zxs_member_held(m->fd)!=0||m->session!=n->session||m->tcon!=n->tcon||
	   m->seq!=n->session->global->auth_session_info_seqnum||n->tcon->compat!=fsp->conn||
	   fsp->vuid!=n->session->global->session_wire_id||fsp->op==NULL||fsp->op->global==NULL||
	   fsp->fsp_flags.is_pathref||!fsp->fsp_flags.is_fsa||
	   (kind!=ZXS_XFER_DELETE&&(fsp->access_mask&SEC_STD_DELETE)==0)||
	   fsp->conn->read_only || memcmp(m->folder.volume,m->member.volume,16)!=0 ||
	   memcmp(m->member.pool,n->tcon->zxs_tree_choice->pool,16)!=0)return false;
	if(kind==ZXS_XFER_RESTORE){
		if(!zxs_folder_retained(&m->folder)||zxs_folder_round(&m->folder)||m->writable||fsp->fsp_flags.closing||
		   (fsp->conn->share_access&SEC_DIR_ADD_FILE)==0)return false;
	}else if(kind==ZXS_XFER_DELETE){
		if(m->folder.mode!=2||m->folder.volume_ordinal<2||!m->recycle_close||!fsp->fsp_flags.closing)return false;
	}else if(kind==ZXS_XFER_PROBE){
		if(m->folder.mode!=2||m->folder.volume_ordinal<2||fsp->fsp_flags.closing)return false;
	}else{return false;}
	for(size_t i=0;i<8;i++){dev=(dev<<8)|m->member.pool[i];ext=(ext<<8)|m->member.pool[i+8];}
	return fsp->file_id.devid==dev&&fsp->file_id.extid==ext&&fsp->file_id.inode==m->member.number&&
	       change_to_user_and_service(fsp->conn,fsp->vuid);
}
static int zxs_xfer_release(struct zxs_xfer_state *s)
{
	TALLOC_FREE(s->watch);
	if(s->answer>=0){close(s->answer);s->answer=-1;}
	if(s->member!=NULL){s->member->busy=false;s->member=NULL;}
	TALLOC_FREE(s->link);return 0;
}
static void zxs_xfer_finish(struct tevent_req *req,NTSTATUS status,bool unknown)
{
	struct zxs_xfer_state *s=tevent_req_data(req,struct zxs_xfer_state);
	s->unknown=unknown;zxs_xfer_release(s);
	if(tevent_req_nterror(req,status))return;
	tevent_req_done(req);
}
static void zxs_xfer_ready(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct zxs_xfer_state *s=tevent_req_data(req,struct zxs_xfer_state);
	int err=0;uint8_t result=0;const bool ready=wait_for_read_recv(sub,&err);
	s->watch=NULL;TALLOC_FREE(sub);
	if(!ready||!zxs_xfer_current(s->native,s->fsp,s->member,s->kind)||
	   zxs_xfer_read(s->answer,s->member->member.file,&result,s->receipt)!=0||result==ZXS_XFER_UNKNOWN){
		zxs_xfer_finish(req,NT_STATUS_OK,true);return;
	}
	NTSTATUS status=NT_STATUS_ACCESS_DENIED;
	switch(result){
	case 0:status=NT_STATUS_OK;break;
	case ZXS_XFER_BUSY:status=NT_STATUS_SHARING_VIOLATION;break;
	case ZXS_XFER_FULL:status=NT_STATUS_DISK_FULL;break;
	case ZXS_XFER_COLLISION:status=NT_STATUS_OBJECT_NAME_COLLISION;break;
	case ZXS_XFER_NOT_FOUND:status=NT_STATUS_OBJECT_NAME_NOT_FOUND;break;
	case ZXS_XFER_UNAVAILABLE:status=NT_STATUS_DEVICE_NOT_READY;break;
	default:break;
	}
	zxs_xfer_finish(req,status,false);
}
_PUBLIC_
struct tevent_req *zxs_xfer_send(TALLOC_CTX *ctx,struct tevent_context *ev,struct tevent_req *outer,
	struct smbd_smb2_request *n,struct files_struct *fsp,uint8_t kind,const char *folder,const char *name,const uint8_t mark[16])
{
	struct tevent_req *req;struct zxs_xfer_state *s;
	req=tevent_req_create(ctx,&s,struct zxs_xfer_state);if(req==NULL)return NULL;
	s->answer=-1;s->kind=kind;s->native=n;s->fsp=fsp;
	talloc_set_destructor(s,zxs_xfer_release);
	struct zxs_read_fsp *m=zxs_find_read(fsp);
	if(!zxs_xfer_current(n,fsp,m,kind)||m->busy){
		tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return tevent_req_post(req,ev);
	}
	s->link=aio_add_req_to_fsp(fsp,outer);
	if(tevent_req_nomem(s->link,req))return tevent_req_post(req,ev);
	talloc_steal(s,s->link);s->member=m;m->busy=true;
	s->answer=zxs_xfer_query(m->fd,kind,(const uint8_t *)folder,folder!=NULL?strlen(folder):0,
				(const uint8_t *)name,name!=NULL?strlen(name):0,mark);
	if(s->answer<0){
		zxs_xfer_finish(req,NT_STATUS_ACCESS_DENIED,false);return tevent_req_post(req,ev);
	}
	if(smbd_smb2_is_compound(n)&&!smbd_smb2_is_last_in_compound(n)){
		smb2_request_set_async_internal(n,true);
	}
	/* After the packet is sent, no fixed time limit on legitimate disk IO.
	 * A closed native request withdraws the actual private endpoint. */
	s->watch=wait_for_read_send(s,ev,s->answer,false);
	if(s->watch==NULL){
		zxs_xfer_finish(req,NT_STATUS_OK,true);return tevent_req_post(req,ev);
	}
	tevent_req_set_callback(s->watch,zxs_xfer_ready,req);return req;
}
_PUBLIC_
NTSTATUS zxs_xfer_recv(struct tevent_req *req,bool *unknown,uint8_t receipt[16])
{
	struct zxs_xfer_state *s=tevent_req_data(req,struct zxs_xfer_state);NTSTATUS status;
	*unknown=s->unknown;
	if(receipt!=NULL)memcpy(receipt,s->receipt,16);
	if(tevent_req_is_nterror(req,&status))return status;
	return NT_STATUS_OK;
}
static bool zxs_xfer_label(const char *name,size_t max)
{
	size_t n=strlen(name);
	return n>0&&n<=max&&name[n-1]!='.'&&name[n-1]!=' '&&strpbrk(name,"\\/:*?\"<>|")==NULL;
}
_PUBLIC_
bool zxs_restore_input(TALLOC_CTX *ctx,DATA_BLOB input,char **folder,char **name)
{
	char *path=NULL,*split;size_t decoded=0;
	if(input.length<20||input.data==NULL||CVAL(input.data,0)!=0||BVAL(input.data,8)!=0)return false;
	const size_t bytes=IVAL(input.data,16);
	/* Reserved bytes and tail padding are not part of the declared name. */
	if(bytes==0||(bytes&1)!=0||bytes>input.length-20||bytes>2*(ZXS_DIR_NAME+1+ZXS_FILE_NAME)||
	   !convert_string_talloc(ctx,CH_UTF16,CH_UNIX,input.data+20,bytes,&path,&decoded)||
	   path==NULL||strlen(path)!=decoded)return false;
	split=strchr(path,'\\');if(split==NULL)return false;
	*split=0;
	if(!zxs_xfer_label(path,ZXS_DIR_NAME)||!zxs_xfer_label(split+1,ZXS_FILE_NAME))return false;
	*folder=path;*name=split+1;return true;
}
