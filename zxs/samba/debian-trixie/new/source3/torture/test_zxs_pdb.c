/* SPDX-License-Identifier: GPL-3.0-or-later
 * Pinned Debian ABI qualification. No provisioned instance, SMB session,
 * database, host account operation or password is needed by this fixture.
 * The table included below is exactly the one installed by the dispatcher. */
#include "includes.h"
#include "passdb.h"
#include "param/zxs_runtime.h"
#include "passdb/pdb_zxs_sqlite_table.h"

static NTSTATUS not_the_backend(struct pdb_methods **out, const char *location)
{
	return NT_STATUS_UNSUCCESSFUL;
}

int main(void)
{
	struct pdb_methods table = zxs_sqlite_methods;
	struct pdb_methods *out = &table;
	const struct pdb_init_function_entry *entry;
	const char *wrong[] = {"tdbsam", "smbpasswd", "zxs_sqlite:",
			       " zxs_sqlite", "zxs_sqlite:/elsewhere", NULL};
	uint32_t rid = 789, policy = 456;
	uint8_t unchanged = 37;
	DATA_BLOB marker = {.data = &unchanged, .length = 1};
	size_t i;

	for (i = 0; i < ARRAY_SIZE(wrong); i++) {
		if (NT_STATUS_IS_OK(make_pdb_method_name(&out, wrong[i])) ||
		    out != NULL) {
			return 1;
		}
	}
	if (NT_STATUS_IS_OK(smb_register_passdb(PASSDB_INTERFACE_VERSION,
					       "zxs_sqlite", not_the_backend)) ||
	    NT_STATUS_IS_OK(smb_register_passdb(PASSDB_INTERFACE_VERSION,
					       "tdbsam", not_the_backend)) ||
	    !NT_STATUS_EQUAL(make_pdb_method_name(&out, "zxs_sqlite"),
			     NT_STATUS_ACCESS_DENIED) || out != NULL) {
		return 1;
	}
	entry = pdb_get_backends();
	if (entry == NULL || entry->next != NULL ||
	    strcmp(entry->name, "zxs_sqlite") != 0 ||
	    NT_STATUS_IS_OK(entry->init(&out, "")) || out != NULL) {
		return 1;
	}

	if (!NT_STATUS_EQUAL(table.getsampwnam(&table, NULL, "zxs-smb-client"),
			     NT_STATUS_NO_SUCH_USER) ||
	    !NT_STATUS_EQUAL(table.getsampwnam(&table, NULL, "nobody"),
			     NT_STATUS_NO_SUCH_USER) ||
	    !NT_STATUS_EQUAL(table.getsampwsid(&table, NULL, NULL),
			     NT_STATUS_NO_SUCH_USER) ||
	    NT_STATUS_IS_OK(table.create_user(&table, NULL, "unused", 0, &rid)) ||
	    table.new_rid(&table, &rid) || rid != 789 ||
	    NT_STATUS_IS_OK(table.get_account_policy(&table,
				PDB_POLICY_LOCK_ACCOUNT_DURATION, &policy)) || policy != 456 ||
	    NT_STATUS_IS_OK(table.get_secret(&table, NULL, "unused", &marker,
					    NULL, NULL, NULL, NULL)) ||
	    marker.data != &unchanged || marker.length != 1 || unchanged != 37 ||
	    table.capabilities(&table) != 0 ||
	    table.is_responsible_for_our_sam(&table) ||
	    table.is_responsible_for_builtin(&table) ||
	    table.is_responsible_for_unix_users(&table) ||
	    table.is_responsible_for_unix_groups(&table)) {
		return 1;
	}
	return 0;
}
