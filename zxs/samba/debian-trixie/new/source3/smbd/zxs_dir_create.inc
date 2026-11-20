/* SPDX-License-Identifier: GPL-3.0-or-later */
static int zxs_create_drop_route(struct smbd_smb2_create_state *s)
{
	TALLOC_FREE(s->zxs_wait);
	TALLOC_FREE(s->zxs_link);
	if(s->zxs_provisional){
		s->zxs_provisional=false;
		if(s->result!=NULL)(void)close_file_free(s->smb1req,&s->result,ERROR_CLOSE);
	}
	return zxs_smb_root_release(&s->zxs_route);
}
static bool zxs_create_without_reply(struct tevent_req *req)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	return s->zxs_unknown;
}
static void zxs_create_unknown(struct tevent_req *req)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	s->zxs_unknown=true;
	zxs_create_drop_route(s);
	/* Teardown may be walking the outer requests' native linked list.
	 * The outer completion detaches only this request, on a later turn. */
	tevent_req_defer_callback(req,s->smb2req->xconn->client->raw_ev_ctx);
	tevent_req_done(req);
}
static bool zxs_create_wait_cancel(struct tevent_req *req)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	if(s->zxs_wait==NULL)return false;
	if(s->zxs_acknowledged){
		if(zxs_smb2_birth_cancel_advisory(s->smb2req,true))return false;
		zxs_create_unknown(req);return true;
	}
	zxs_create_drop_route(s);
	tevent_req_defer_callback(req,s->smb2req->xconn->client->raw_ev_ctx);
	tevent_req_nterror(req,NT_STATUS_CANCELLED);return true;
}
static void zxs_create_success(struct tevent_req *req,uint32_t action)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	s->op=s->result->op;s->info=action;
	NTSTATUS status;
	if(s->zxs_reserved_context.data!=NULL &&
	   !zxs_qfid_response(s->zxs_reserved_context,zxs_wire_file_number(s->result))){
		(void)close_file_free(s->smb1req,&s->result,ERROR_CLOSE);
		status=NT_STATUS_INTERNAL_ERROR;
	}else{status=smbd_smb2_create_after_exec(req);}
	if(action==FILE_WAS_CREATED && !NT_STATUS_IS_OK(status)){
		zxs_create_unknown(req);return;
	}
	if(tevent_req_nterror(req,status))return;
	smbd_smb2_create_finish(req); /* terminal */
}
static bool zxs_directory_committed(struct smbd_smb2_create_state *s)
{
	struct zxs_dir_search *d=zxs_find_dir(s->result);
	struct smbd_smb2_request *n=s->smb2req;
	return d!=NULL && zxs_folder_ordinary_child(&d->folder) &&
		NT_STATUS_IS_OK(n->xconn->transport.status) &&
		zxs_request_root_current(n) && d->tcon==n->tcon && d->session==n->session &&
		d->seq==n->session->global->auth_session_info_seqnum &&
		s->result->conn==n->tcon->compat && s->result->vuid==d->session->global->session_wire_id &&
		change_to_user_and_service(s->result->conn,s->result->vuid) &&
		zxs_odir_read_done(d->folder.root_fd,d->folder.directory,d->folder.directory_op)==0 &&
		zxs_request_folder_current(n,&d->folder);
}
static void zxs_birth_confirmed(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_read_fsp *f=zxs_find_read(s->result);
	int err=0;bool ready=wait_for_read_recv(sub,&err);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(!ready || !s->zxs_provisional || (s->zxs_is_dir?!zxs_directory_committed(s):(f==NULL || f->fd<0 ||
	   !NT_STATUS_IS_OK(s->smb2req->xconn->transport.status) ||
	   !zxs_request_folder_current(s->smb2req,&f->folder) || f->tcon!=s->smb2req->tcon ||
	   f->session!=s->smb2req->session || f->seq!=s->smb2req->session->global->auth_session_info_seqnum ||
	   s->result->conn!=s->smb2req->tcon->compat ||
	   s->result->vuid!=f->session->global->session_wire_id ||
	   !change_to_user_and_service(s->result->conn,s->result->vuid) ||
	   zxs_birth_read_done(f->fd,f->member.file)!=0 || zxs_member_held(f->fd)!=0))){
		/* After ACK, EOF or late revocation is unknown to the native
		 * sender, not proof that the original birth did not commit. */
		zxs_create_unknown(req);return;
	}
	TALLOC_FREE(s->zxs_link);s->zxs_provisional=false;s->result->fsp_flags.closing=false;
	zxs_create_success(req,FILE_WAS_CREATED);
}
static void zxs_create_open(struct tevent_req *req,const struct zxs_member_info *member,int *fd,bool offered)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_smb_tree_cut *folder=s->zxs_route.root_fd>=0?&s->zxs_route:NULL;
	uint32_t may_add=s->zxs_is_dir?SEC_DIR_ADD_SUBDIR:SEC_DIR_ADD_FILE;
	if(offered && (folder==NULL || folder->mode!=2 || s->smb1req->conn->read_only ||
	              (s->smb1req->conn->share_access&may_add)==0 ||
	              (s->zxs_is_dir && !zxs_folder_ordinary_child(folder)))){
		if(fd!=NULL&&*fd>=0){close(*fd);*fd=-1;}
		tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	NTSTATUS status=zxs_open_member_fsp(s->smb2req,s->smb1req,s->zxs_access,s->zxs_share,
		s->zxs_native_path,member,folder,s->zxs_writer,fd,&s->result);
	if(fd!=NULL && *fd>=0){close(*fd);*fd=-1;}
	if(!NT_STATUS_IS_OK(status) && open_was_deferred(s->smb1req->xconn,s->smb1req->mid)){
		SMBPROFILE_IOBYTES_ASYNC_SET_IDLE(s->smb2req->profile);return;
	}
	if(tevent_req_nterror(req,status))return;
	if(!offered){zxs_create_success(req,FILE_WAS_OPENED);return;}
	/* The original native share-mode entry already exists. The reserved
	 * OPEN ID is not usable until the affirmative FILE receipt. */
	s->result->fsp_flags.closing=true;s->zxs_provisional=true;
	struct zxs_read_fsp *f=zxs_find_read(s->result);
	struct zxs_dir_search *d=zxs_find_dir(s->result);
	if(s->zxs_is_dir?(d==NULL || !zxs_folder_ordinary_child(&d->folder)):f==NULL)goto reject;
	int peer=s->zxs_is_dir?d->folder.root_fd:f->fd;
	s->zxs_link=aio_add_req_to_fsp(s->result,req);
	if(s->zxs_link==NULL)goto reject;
	talloc_steal(s,s->zxs_link);
	s->zxs_wait=wait_for_read_send(s,s->ev,peer,false);
	if(s->zxs_wait==NULL || (s->zxs_is_dir?
	   zxs_odir_ack(peer,d->folder.directory,d->folder.directory_op):zxs_birth_ack(peer,f->member.file))!=0)goto reject;
	s->zxs_acknowledged=true;
	tevent_req_set_callback(s->zxs_wait,zxs_birth_confirmed,req);return;
reject:
	zxs_create_drop_route(s);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);
}
static void zxs_dir_opened(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_member_info member;
	int fd=-1;bool offer=false;
	NTSTATUS status=s->zxs_is_dir?zxs_dir_cut_recv(sub,NULL):zxs_member_cut_recv(sub,&member,&fd,&offer);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(tevent_req_nterror(req,status))return;
	if((offer && s->zxs_disposition==FILE_OPEN) || (!offer && s->zxs_disposition==FILE_CREATE)){
		if(fd>=0)close(fd);
		tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	zxs_create_open(req,s->zxs_is_dir?NULL:&member,&fd,offer);
}
static void zxs_create_round_ready(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_smb_tree_cut round;
	NTSTATUS status=zxs_folder_cut_recv(sub,&round);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(tevent_req_nterror(req,status))return;
	if(zxs_r_round_own_parent(&round,&s->zxs_route)!=0){
		zxs_smb_root_release(&round);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	s->zxs_route=round;
	zxs_create_open(req,NULL,NULL,false);
}
static void zxs_create_ordinary_file(struct tevent_req *req)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	uint32_t requested=s->zxs_access;
	se_map_generic(&requested,&file_generic_mapping);
	if(s->zxs_disposition!=FILE_OPEN && (requested&SEC_STD_DELETE)!=0){
		tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	NTSTATUS status=zxs_open_access(s->smb1req->conn,false,&s->zxs_route,&s->zxs_access,s->zxs_share);
	if(tevent_req_nterror(req,status))return;
	if(s->zxs_disposition!=FILE_OPEN)s->zxs_access&=~SEC_STD_DELETE;
	s->zxs_writer=(s->zxs_access&(SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA))!=0;
	unsigned action=s->zxs_writer?ZXS_MEMBER_WRITE_OPEN:ZXS_MEMBER_OPEN;
	if(s->zxs_disposition!=FILE_OPEN){
		action=ZXS_FILE_BIRTH+(s->zxs_writer?2:0)+(s->zxs_disposition==FILE_OPEN_IF?1:0);
	}
	s->zxs_wait=zxs_dir_cut_send(s,s->ev,s->smb2req,&s->zxs_route,action,
		(const uint8_t *)s->zxs_name,strlen(s->zxs_name));
	if(tevent_req_nomem(s->zxs_wait,req))return;
	tevent_req_set_callback(s->zxs_wait,zxs_dir_opened,req);
}
static void zxs_create_ordinary_ready(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	struct zxs_smb_tree_cut next;bool offer=false;
	NTSTATUS status=zxs_odir_cut_recv(sub,&next,&offer);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(tevent_req_nterror(req,status))return;
	if((offer && (!s->zxs_is_dir || s->zxs_disposition==FILE_OPEN)) ||
	   (!offer && s->zxs_is_dir && s->zxs_disposition==FILE_CREATE) ||
	   zxs_odir_own_parent(&next,&s->zxs_route)!=0){
		zxs_smb_root_release(&next);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
	}
	s->zxs_route=next;
	if(s->zxs_is_dir)zxs_create_open(req,NULL,NULL,offer);
	else zxs_create_ordinary_file(req);
}
static bool zxs_retained_name(const char *path,bool directory)
{
	const char *split=strchr(path,'\\');
	if(directory)return split==NULL && zxs_delete_hex((const uint8_t *)path,strlen(path));
	return split!=NULL && zxs_delete_hex((const uint8_t *)path,(size_t)(split-path)) &&
		zxs_odir_component(split+1,strlen(split+1),ZXS_FILE_NAME);
}
static void zxs_create_folder_ready(struct tevent_req *sub)
{
	struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	NTSTATUS status=zxs_folder_cut_recv(sub,&s->zxs_route);
	s->zxs_wait=NULL;TALLOC_FREE(sub);
	if(tevent_req_nterror(req,status))return;
	if(zxs_folder_retained(&s->zxs_route)){
		if(s->zxs_disposition!=FILE_OPEN || (!s->zxs_nested && !s->zxs_is_dir)){
			tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
		}
		if(!s->zxs_nested){zxs_create_open(req,NULL,NULL,false);return;}
		if(!zxs_retained_name(s->zxs_relative,s->zxs_is_dir)){
			tevent_req_nterror(req,NT_STATUS_OBJECT_PATH_INVALID);return;
		}
		s->zxs_name=s->zxs_relative;
		if(s->zxs_is_dir){
			s->zxs_wait=zxs_dir_cut_send(s,s->ev,s->smb2req,&s->zxs_route,ZXS_RETAINED_DIR,
				(const uint8_t *)s->zxs_name,strlen(s->zxs_name));
			if(tevent_req_nomem(s->zxs_wait,req))return;
			tevent_req_set_callback(s->zxs_wait,zxs_create_round_ready,req);return;
		}
		status=zxs_open_access(s->smb1req->conn,false,&s->zxs_route,&s->zxs_access,s->zxs_share);
		if(tevent_req_nterror(req,status))return;
		const unsigned retained=(s->zxs_share&FILE_SHARE_DELETE)?ZXS_RETAINED_SHARE_DELETE:ZXS_RETAINED_OPEN;
		s->zxs_wait=zxs_dir_cut_send(s,s->ev,s->smb2req,&s->zxs_route,retained,
			(const uint8_t *)s->zxs_name,strlen(s->zxs_name));
		if(tevent_req_nomem(s->zxs_wait,req))return;
		tevent_req_set_callback(s->zxs_wait,zxs_dir_opened,req);return;
	}
	if(!s->zxs_nested){
		if(s->zxs_is_dir)zxs_create_open(req,NULL,NULL,false);
		else zxs_create_ordinary_file(req);
		return;
	}
	const char *relative=s->zxs_relative;
	size_t width=s->zxs_is_dir?strlen(relative):(size_t)(s->zxs_name-relative-1);
	uint8_t mode=0;
	if(s->zxs_is_dir){
		bool can_make=s->zxs_route.mode==2 && !s->smb1req->conn->read_only &&
			(s->smb1req->conn->share_access&SEC_DIR_ADD_SUBDIR)!=0;
		if(s->zxs_disposition==FILE_CREATE && !can_make){
			tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return;
		}
		mode=s->zxs_disposition==FILE_CREATE?2:
			(s->zxs_disposition==FILE_OPEN_IF && can_make)?3:1;
	}
	s->zxs_wait=zxs_odir_cut_send(s,s->ev,s->smb2req,&s->zxs_route,mode,(const uint8_t *)relative,width);
	if(tevent_req_nomem(s->zxs_wait,req))return;
	tevent_req_set_callback(s->zxs_wait,zxs_create_ordinary_ready,req);
}
static bool zxs_create_select(struct tevent_req *req,const char *name,bool directory)
{
	struct smbd_smb2_create_state *s=tevent_req_data(req,struct smbd_smb2_create_state);
	const char *split=strchr(name,'\\');
	s->zxs_is_dir=directory;s->zxs_route.root_fd=-1;s->zxs_native_path=name;
	talloc_set_destructor(s,zxs_create_drop_route);
	if(directory && name[0]==0){
		if(s->zxs_disposition==FILE_CREATE){
			tevent_req_nterror(req,NT_STATUS_OBJECT_NAME_COLLISION);return false;
		}
		s->zxs_name="";
		s->zxs_wait=zxs_dir_cut_send(s,s->ev,s->smb2req,NULL,0,NULL,0);
		if(tevent_req_nomem(s->zxs_wait,req))return false;
		tevent_req_set_callback(s->zxs_wait,zxs_dir_opened,req);
	}else{
		size_t length=split==NULL?strlen(name):(size_t)(split-name);
		const char *relative=split==NULL?NULL:split+1;
		const char *last=relative==NULL?NULL:strrchr(relative,'\\');
		s->zxs_nested=relative!=NULL && (directory || last!=NULL);
		if(!zxs_odir_component(name,length,ZXS_DIR_NAME) ||
		   (relative==NULL && !directory) ||
		   (relative!=NULL && (strlen(relative)>ZXS_OD_PATH_MAX || !zxs_odir_relative(relative,directory)))){
			tevent_req_nterror(req,NT_STATUS_OBJECT_PATH_INVALID);return false;
		}
		if(relative==NULL && s->zxs_disposition==FILE_CREATE){
			tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return false;
		}
		s->zxs_relative=relative;
		s->zxs_name=relative==NULL?name:(last==NULL?relative:last+1);
		uint32_t mask=s->zxs_access;
		se_map_generic(&mask,&file_generic_mapping);
		/* Distinct storage scheduling capacity: an explicit compatible
		 * read never competes for the writers' folder admission slots. */
		unsigned access=ZXS_NS_OPEN;
		if(directory && relative!=NULL && s->zxs_disposition!=FILE_OPEN){
			const bool add=!s->smb1req->conn->read_only &&
				(s->smb1req->conn->share_access&SEC_DIR_ADD_SUBDIR)!=0;
			if(!add && s->zxs_disposition==FILE_CREATE){
				tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return false;
			}
			if(add)access=s->zxs_disposition==FILE_CREATE?ZXS_NS_WRITE:ZXS_NS_MAXIMUM;
		}else if(!directory){
			const uint32_t bits=SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA;
			const connection_struct *conn=s->smb1req->conn;
			if((mask&bits)!=0){
				if(conn->read_only || (mask&bits&~conn->share_access)!=0){
					tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return false;
				}
				access=ZXS_NS_WRITE;
			}else if((mask&SEC_FLAG_MAXIMUM_ALLOWED)!=0 && !conn->read_only && (conn->share_access&bits)!=0){
				access=ZXS_NS_MAXIMUM;
			}
		}
		s->zxs_wait=zxs_dir_cut_send(s,s->ev,s->smb2req,NULL,access,
			(const uint8_t *)name,length);
		if(tevent_req_nomem(s->zxs_wait,req))return false;
		tevent_req_set_callback(s->zxs_wait,zxs_create_folder_ready,req);
	}
	tevent_req_set_cancel_fn(req,zxs_create_wait_cancel);
	smb2_request_set_async_internal(s->smb2req,true);
	return true;
}
