/* SPDX-License-Identifier: GPL-3.0-or-later
 * Included beside the required-static, complete ABI 50 rejection table.
 * Only explicit implementations may sit above it; never a disk stack. */
static int zxs_file_connect(vfs_handle_struct *handle,
			    const char *service, const char *user)
{
	/* Neither of these strings proves the original SMB session or a SQL
	 * tree. No authenticated handoff yet: do not run the host connect. */
	(void)handle; (void)service; (void)user;
	errno = EACCES;
	return -1;
}

static const struct vfs_fn_pointers zxs_file_front = {
	.connect_fn = zxs_file_connect,
};

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
