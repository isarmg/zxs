/* SPDX-License-Identifier: GPL-3.0-or-later
 * Only an empty-name, read-only FILE_OPEN after the live TCON's original
 * positive storage cut. Construct a real protocol handle; no disk open. */
NTSTATUS zxs_open_dir_fsp(struct smbd_smb2_request *n,struct smb_request *req,
			  uint32_t desired,uint32_t share,struct files_struct **out)
{
	struct open_ntcreate_lock_state lock={0};
	struct files_struct *fsp=NULL;
	struct smb_filename *name=NULL;
	struct zxs_dir_search *d=NULL;
	struct deferred_open_record *prior=NULL;
	struct zxs_smb_tree_cut *root;
	struct stat_ex st={0};
	struct timespec mtime=make_omit_timespec();
	uint32_t allowed;
	uint64_t dev=0,ext=0;
	NTSTATUS status;
	const bool deferred=get_deferred_open_message_state(req,NULL,&prior);
	if(deferred){remove_deferred_open_message_smb(req->xconn,req->mid);}
	if(!zxs_request_root_current(n) || req->conn!=n->tcon->compat ||
	   req->vuid!=n->session->global->session_wire_id ||
	   !change_to_user_and_service(req->conn,req->vuid)){return NT_STATUS_ACCESS_DENIED;}
	root=n->tcon->zxs_tree_choice;
	allowed=req->conn->share_access & (SEC_RIGHTS_DIR_READ|SEC_DIR_TRAVERSE);
	se_map_generic(&desired,&file_generic_mapping);
	if((desired & ~(allowed|SEC_FLAG_MAXIMUM_ALLOWED))!=0){return NT_STATUS_ACCESS_DENIED;}
	if(desired & SEC_FLAG_MAXIMUM_ALLOWED){desired=allowed;}
	if((share & ~(FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE))!=0){return NT_STATUS_INVALID_PARAMETER;}
	for(size_t i=0;i<8;i++){dev=(dev<<8)|root->pool[i];ext=(ext<<8)|root->pool[8+i];}
	st.st_ex_dev=dev;st.st_ex_ino=(uint64_t)root->volume_ordinal+1;
	st.st_ex_mode=S_IFDIR|0500;st.st_ex_nlink=1;
	status=file_new(req,req->conn,&fsp);
	if(!NT_STATUS_IS_OK(status))return status;
	name=synthetic_smb_fname(fsp,".",NULL,&st,0,0);
	if(name==NULL){status=NT_STATUS_NO_MEMORY;goto fail;}
	status=fsp_set_smb_fname(fsp,name);TALLOC_FREE(name);
	if(!NT_STATUS_IS_OK(status))goto fail;
	fsp->fsp_name->st.cached_dos_attributes=FILE_ATTRIBUTE_DIRECTORY;
	fsp->vuid=req->vuid;fsp->file_pid=req->smbpid;fsp->access_mask=desired;
	fsp->fsp_flags.is_directory=true;fsp->fsp_flags.can_read=false;
	fsp->fsp_flags.can_write=false;fsp->fsp_flags.can_lock=false;
	fsp->file_id=(struct file_id){.devid=dev,.inode=st.st_ex_ino,.extid=ext};
	fsp->oplock_type=NO_OPLOCK;fsp->sent_oplock_break=NO_BREAK_SENT;
	d=vfs_add_fsp_extension_notype(req->conn->vfs_handles,fsp,sizeof(*d),NULL);
	if(d==NULL){status=NT_STATUS_NO_MEMORY;goto fail;}
	*d=(struct zxs_dir_search){.session=n->session,.tcon=n->tcon,.seq=n->session->global->auth_session_info_seqnum};
	lock.fsp=fsp;lock.object_type="directory";lock.req=req;lock.create_disposition=FILE_OPEN;
	lock.access_mask=desired;lock.open_access_mask=desired;lock.share_access=share;
	lock.oplock_request=NO_OPLOCK;lock.first_open_attempt=!deferred;
	status=share_mode_entry_prepare_lock_add(&lock.prepare_state,fsp->file_id,
		req->conn->connectpath,fsp->fsp_name,&mtime,open_ntcreate_lock_add_entry,&lock);
	if(!NT_STATUS_IS_OK(status))goto fail;
	if(!NT_STATUS_IS_OK(lock.status)){status=lock.status;goto fail;}
	status=share_mode_entry_prepare_unlock(&lock.prepare_state,NULL,&lock);
	if(!NT_STATUS_IS_OK(status)){smb_panic("zxs root share_mode_entry_prepare_unlock");}
	fsp->fsp_flags.is_fsa=true;
	*out=fsp;return NT_STATUS_OK;
fail:
	file_free(req,fsp);return status;
}
