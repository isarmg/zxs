/* SPDX-License-Identifier: GPL-3.0-or-later
 * Only an admitted FILE_OPEN after the live TCON's original
 * positive storage cut. Construct a real protocol handle; no disk open. */
static void zxs_read_fsp_end(void *v)
{
	struct zxs_read_fsp *f=v;if(f->fd>=0){close(f->fd);f->fd=-1;}
	(void)zxs_smb_root_release(&f->folder);
}
static void zxs_dir_fsp_end(void *v)
{
	struct zxs_dir_search *d=v;(void)zxs_smb_root_release(&d->folder);
}
NTSTATUS zxs_open_access(connection_struct *c,bool directory,const struct zxs_smb_tree_cut *folder,uint32_t *desired,uint32_t share)
{
	uint32_t profile=directory?(SEC_RIGHTS_DIR_READ|SEC_DIR_TRAVERSE):SEC_RIGHTS_FILE_READ;
	if(!c->read_only && folder!=NULL && folder->mode==2){
		if(directory)profile|=SEC_DIR_ADD_FILE|SEC_DIR_ADD_SUBDIR;
		else profile|=SEC_RIGHTS_FILE_WRITE|SEC_STD_DELETE;
	}
	/* Retained DELETE access is for the exact FILE_RENAME_INFORMATION
	 * restore only. No retained disposition/create/EOF/write is admitted. */
	if(!directory && !c->read_only && zxs_folder_retained(folder)){profile|=SEC_STD_DELETE;}
	uint32_t allowed=c->share_access & profile;
	se_map_generic(desired,&file_generic_mapping);
	if((*desired & ~(allowed|SEC_FLAG_MAXIMUM_ALLOWED))!=0)return NT_STATUS_ACCESS_DENIED;
	if(*desired & SEC_FLAG_MAXIMUM_ALLOWED)*desired=allowed;
	if((share & ~(FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE))!=0)return NT_STATUS_INVALID_PARAMETER;
	return NT_STATUS_OK;
}
/* Query the original FSP's policy, not the rights requested or granted
 * to this specific handle: later operations must still obtain their
 * own grant. The surrounding CREATE already verified the live original. */
_PUBLIC_
bool zxs_fsp_maximum_open_access(const struct files_struct *f,uint32_t *allowed)
{
	if(f==NULL || f->fsp_flags.closing)return false;
	const bool directory=f->fsp_flags.is_directory;
	const struct zxs_smb_tree_cut *folder=NULL;
	if(directory){
		const struct zxs_dir_search *d=zxs_find_dir(f);
		if(d==NULL)return false;
		if(d->folder.root_fd>=0)folder=&d->folder;
	}else{
		const struct zxs_read_fsp *r=zxs_find_read(f);
		if(r==NULL || r->folder.root_fd<0)return false;
		folder=&r->folder;
	}
	*allowed=SEC_FLAG_MAXIMUM_ALLOWED;
	return NT_STATUS_IS_OK(zxs_open_access(f->conn,directory,folder,allowed,0));
}
NTSTATUS zxs_open_member_fsp(struct smbd_smb2_request *n,struct smb_request *req,
			  uint32_t desired,uint32_t share,const char *display,const struct zxs_member_info *member,
			  struct zxs_smb_tree_cut *folder,bool writable,int *owned,struct files_struct **out)
{
	struct open_ntcreate_lock_state lock={0};
	struct files_struct *fsp=NULL;
	struct smb_filename *name=NULL;
	struct zxs_dir_search *d=NULL;
	struct deferred_open_record *prior=NULL;
	const struct zxs_smb_tree_cut *root;
	struct stat_ex st={0};
	struct timespec mtime=make_omit_timespec();
	const bool directory=member==NULL;
	uint64_t dev=0,ext=0;
	NTSTATUS status;
	const bool deferred=get_deferred_open_message_state(req,NULL,&prior);
	if(deferred){remove_deferred_open_message_smb(req->xconn,req->mid);}
	if(!zxs_request_root_current(n) || req->conn!=n->tcon->compat ||
	   req->vuid!=n->session->global->session_wire_id ||
	   !change_to_user_and_service(req->conn,req->vuid)){return NT_STATUS_ACCESS_DENIED;}
	root=folder!=NULL?folder:n->tcon->zxs_tree_choice;
	if(display==NULL || (display[0]==0)!=(folder==NULL) ||
	   (folder!=NULL && !zxs_request_folder_current(n,folder)) ||
	   (!directory && (folder==NULL || owned==NULL || *owned<0 || zxs_smb_ctl_live(*owned)!=0 ||
	                    memcmp(root->pool,member->pool,16)!=0 || memcmp(root->volume,member->volume,16)!=0)))
		return NT_STATUS_ACCESS_DENIED;
	status=zxs_open_access(req->conn,directory,folder,&desired,share);if(!NT_STATUS_IS_OK(status))return status;
	if(writable!=(bool)(!directory && (desired&(SEC_FILE_WRITE_DATA|SEC_FILE_APPEND_DATA))!=0))return NT_STATUS_ACCESS_DENIED;
	for(size_t i=0;i<8;i++){dev=(dev<<8)|root->pool[i];ext=(ext<<8)|root->pool[8+i];}
	st.st_ex_dev=dev;st.st_ex_ino=directory?(uint64_t)root->volume_ordinal+1:member->number;
	st.st_ex_mode=directory?(S_IFDIR|0500):(S_IFREG|(writable?0600:0400));st.st_ex_nlink=1;
	st.st_ex_size=directory?0:(off_t)member->size;
	status=file_new(req,req->conn,&fsp);
	if(!NT_STATUS_IS_OK(status))return status;
	name=synthetic_smb_fname(fsp,display[0]==0?".":display,NULL,&st,0,0);
	if(name==NULL){status=NT_STATUS_NO_MEMORY;goto fail;}
	status=fsp_set_smb_fname(fsp,name);TALLOC_FREE(name);
	if(!NT_STATUS_IS_OK(status))goto fail;
	/* ShareAccess stays keyed by the same complete FILE. Keep the native
	 * link-level pending marker disjoint from a retained old read handle
	 * that lives through a restore to the same human name. */
	if(!directory){fsp->name_hash=(fsp->name_hash&UINT32_C(0x7fffffff))|
		(zxs_folder_retained(folder)?UINT32_C(0x80000000):0);}
	fsp->fsp_name->st.cached_dos_attributes=directory?FILE_ATTRIBUTE_DIRECTORY:FILE_ATTRIBUTE_NORMAL;
	fsp->vuid=req->vuid;fsp->file_pid=req->smbpid;fsp->access_mask=desired;
	fsp->fsp_flags.is_directory=directory;fsp->fsp_flags.can_read=!directory && (desired&SEC_FILE_READ_DATA)!=0;
	fsp->fsp_flags.can_write=writable;fsp->fsp_flags.can_lock=!directory;
	fsp->file_id=(struct file_id){.devid=dev,.inode=st.st_ex_ino,.extid=ext};
	if(directory){
		fsp->file_id=zxs_directory_file_id(root);
		if(zxs_folder_ordinary_child(root)||zxs_folder_round(root))fsp->fsp_name->st.st_ex_ino=UINT64_MAX;
	}
	fsp->oplock_type=NO_OPLOCK;fsp->sent_oplock_break=NO_BREAK_SENT;
	if(directory){
		d=vfs_add_fsp_extension_notype(req->conn->vfs_handles,fsp,sizeof(*d),zxs_dir_fsp_end);
		if(d==NULL){status=NT_STATUS_NO_MEMORY;goto fail;}
		*d=(struct zxs_dir_search){.session=n->session,.tcon=n->tcon,.seq=n->session->global->auth_session_info_seqnum,
			.folder.root_fd=-1};
		if(folder!=NULL){
			d->folder=*folder;folder->root_fd=-1;
			folder->owns_parent=false;folder->parent_fd=-1;
		}
	}else{
		struct zxs_read_fsp *f=vfs_add_fsp_extension_notype(req->conn->vfs_handles,fsp,sizeof(*f),zxs_read_fsp_end);
		if(f==NULL){status=NT_STATUS_NO_MEMORY;goto fail;}
		*f=(struct zxs_read_fsp){.session=n->session,.tcon=n->tcon,.seq=n->session->global->auth_session_info_seqnum,
			.member=*member,.fd=*owned,.writable=writable,.folder=*folder};
		*owned=-1;folder->root_fd=-1;folder->owns_parent=false;folder->parent_fd=-1;
	}
	lock.fsp=fsp;lock.object_type=directory?"directory":"file";lock.req=req;lock.create_disposition=FILE_OPEN;
	lock.access_mask=desired;lock.open_access_mask=desired;lock.share_access=share;
	lock.oplock_request=NO_OPLOCK;lock.first_open_attempt=!deferred;
	status=share_mode_entry_prepare_lock_add(&lock.prepare_state,fsp->file_id,
		req->conn->connectpath,fsp->fsp_name,&mtime,open_ntcreate_lock_add_entry,&lock);
	if(!NT_STATUS_IS_OK(status))goto fail;
	if(!NT_STATUS_IS_OK(lock.status)){status=lock.status;goto fail;}
	status=share_mode_entry_prepare_unlock(&lock.prepare_state,NULL,&lock);
	if(!NT_STATUS_IS_OK(status)){smb_panic("zxs virtual share_mode_entry_prepare_unlock");}
	fsp->fsp_flags.is_fsa=true;
	*out=fsp;return NT_STATUS_OK;
fail:
	file_free(req,fsp);return status;
}
