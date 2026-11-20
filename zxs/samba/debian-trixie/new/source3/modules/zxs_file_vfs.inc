/* SPDX-License-Identifier: GPL-3.0-or-later
 * Included beside the required-static, complete ABI 50 rejection table.
 * Only explicit implementations may sit above it; never a disk stack. */
/* Only make_connection_snum's root bootstrap may consume this snapshot.
 * Subsequent operations require a fresh storage request on the original
 * tree, not a stat from these cached fields. No disk VFS sits underneath. */
static const struct zxs_smb_tree_cut *zxs_opening_root(vfs_handle_struct *handle)
{
	connection_struct *c=handle==NULL?NULL:handle->conn;
	struct zxs_smb_tree_cut *root;
	if (c==NULL || c->tcon_done || c->tcon==NULL || c->ipc || c->printer ||
	    !zxs_file_vfs_active(c) || c->connectpath==NULL ||
	    strcmp(c->connectpath,ZXS_SMB_VIRTUAL_ROOT)!=0) {
		errno=EACCES;return NULL;
	}
	root=c->tcon->zxs_tree_choice;
	if (root==NULL || root->root_fd<0 || zxs_smb_ctl_live(root->root_fd)!=0) {
		errno=EACCES;return NULL;
	}
	return root;
}
static int zxs_file_connect(vfs_handle_struct *h,const char *service,const char *user)
{
	uint8_t id[16];
	const struct zxs_smb_tree_cut *r=zxs_opening_root(h);
	if (r==NULL || user==NULL || strcmp(user,"zxs-smb-client")!=0 || service==NULL ||
	    !zxs_smb_service_id((const uint8_t *)service,strlen(service),id) ||
	    memcmp(id,r->share,16)!=0) {errno=EACCES;return -1;}
	return 0;
}
static void zxs_file_disconnect(vfs_handle_struct *h)
{
	if (h!=NULL && h->conn!=NULL && h->conn->tcon!=NULL) {
		zxs_smb_root_release(h->conn->tcon->zxs_tree_choice);
	}
}
static bool zxs_root_name(const struct smb_filename *n)
{
	return n!=NULL && n->base_name!=NULL && strcmp(n->base_name,ZXS_SMB_VIRTUAL_ROOT)==0 &&
	       n->stream_name==NULL && n->twrp==0 && n->flags==0 && n->fsp==NULL;
}
static struct smb_filename *zxs_root_realpath(vfs_handle_struct *h,TALLOC_CTX *ctx,const struct smb_filename *n)
{
	if (!zxs_root_name(n)||zxs_opening_root(h)==NULL) {errno=EACCES;return NULL;}
	return synthetic_smb_fname(ctx,ZXS_SMB_VIRTUAL_ROOT,NULL,NULL,0,0);
}
static int zxs_root_stat(vfs_handle_struct *h,struct smb_filename *n)
{
	const struct zxs_smb_tree_cut *r=zxs_opening_root(h);
	uint64_t device=0;
	if (r==NULL||!zxs_root_name(n)) {errno=EACCES;return -1;}
	for(size_t i=0;i<8;i++) {device=(device<<8)|r->pool[i];}
	ZERO_STRUCT(n->st);
	n->st.st_ex_dev=device;n->st.st_ex_ino=(uint64_t)r->volume_ordinal+1;
	n->st.st_ex_mode=S_IFDIR|0500;n->st.st_ex_nlink=1;
	/* This is not exported FILE metadata; original TCON needs a directory
	 * and the stable virtual device. No actual root timestamps or UID leak. */
	return 0;
}
static uint32_t zxs_root_capabilities(vfs_handle_struct *h,enum timestamp_set_resolution *res)
{
	(void)h;*res=TIMESTAMP_SET_SECONDS;return 0; /* no advertised filesystem extensions */
}
static const struct vfs_fn_pointers zxs_file_front = {
	.connect_fn=zxs_file_connect,.disconnect_fn=zxs_file_disconnect,
	.realpath_fn=zxs_root_realpath,.stat_fn=zxs_root_stat,.fs_capabilities_fn=zxs_root_capabilities,
};

/* Pure pointer checks on the exact installed pair. This runs inside
 * Samba's native share-mode critical section: no VFS callbacks or I/O. */
_PUBLIC_
bool zxs_file_vfs_active(const connection_struct *conn)
{
	const vfs_handle_struct *front, *terminal;
	if (conn == NULL || conn->ipc || conn->printer) {
		return false;
	}
	front = conn->vfs_handles;
	if (front == NULL || front->conn != conn || front->fns != &zxs_file_front) {
		return false;
	}
	terminal = front->next;
	return terminal != NULL && terminal->conn == conn &&
	       terminal->fns == &vfs_not_implemented_fns && terminal->next == NULL;
}

_PUBLIC_
bool zxs_file_vfs_install(connection_struct *conn)
{
	vfs_handle_struct *front, *terminal;
	size_t i;
	uintptr_t bits;
	const uint8_t *raw = (const uint8_t *)&vfs_not_implemented_fns;
	/* In this pinned amd64 ABI the struct consists exclusively of
	 * function pointers. Upstream uses the same layout to check the
	 * table, but its assertion is absent in non-DEVELOPER builds. */
	_Static_assert(sizeof(bits) == sizeof(void (*)(void)) &&
		       sizeof(vfs_not_implemented_fns) % sizeof(bits) == 0,
		       "unexpected native VFS function table layout");
	if (conn == NULL || conn->vfs_handles != NULL || conn->ipc || conn->printer) {
		errno = EACCES;
		return false;
	}
	for (i=0; i<sizeof(vfs_not_implemented_fns); i+=sizeof(bits)) {
		memcpy(&bits, raw+i, sizeof(bits));
		if (bits == 0) {
			errno = ENOSYS;
			return false;
		}
	}
	front = talloc_zero(conn, vfs_handle_struct);
	if (front == NULL) {
		errno = ENOMEM;
		return false;
	}
	terminal = talloc_zero(conn, vfs_handle_struct);
	if (terminal == NULL) {
		TALLOC_FREE(front);
		errno = ENOMEM;
		return false;
	}
	front->conn = conn;
	front->fns = &zxs_file_front;
	terminal->conn = conn;
	terminal->fns = &vfs_not_implemented_fns;
	DLIST_ADD(conn->vfs_handles, terminal);
	DLIST_ADD(conn->vfs_handles, front);
	return true;
}
