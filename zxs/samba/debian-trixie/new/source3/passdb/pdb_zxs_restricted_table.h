/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private overlay for the exact Debian Samba passdb ABI. Included by its
 * native dispatcher and the source-only ABI fixture. This is not a generic
 * password query interface: the only SMB credential belongs to native tdbsam.
 * Never start with make_pdb_method(): its "defaults" can alter UNIX users,
 * groups, policy or Samba's secrets store. All 65 operation slots below
 * have their real C prototype; there are no casted or NULL deny callbacks. */
#ifndef ZXS_MANAGED_PDB_TABLE_H
#define ZXS_MANAGED_PDB_TABLE_H

#if PASSDB_INTERFACE_VERSION != 25
#error "Re-audit every struct pdb_methods callback for this private build"
#endif

/* One definition per ABI slot. Unsupported operations never consult a
 * host service, touch their arguments, disclose a verifier or mint a token.
 * The only possible true describes SAM routing, not any account or session. */
#define ZXS_PDB_FIELDS(X) \
	X(struct pdb_domain_info *, get_domain_info, NULL, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx)) \
	X(NTSTATUS, getsampwnam, NT_STATUS_NO_SUCH_USER, \
	  (struct pdb_methods *p, struct samu *sam_acct, const char *username)) \
	X(NTSTATUS, getsampwsid, NT_STATUS_NO_SUCH_USER, \
	  (struct pdb_methods *p, struct samu *sam_acct, const struct dom_sid *sid)) \
	X(NTSTATUS, create_user, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *tmp_ctx, const char *name, uint32_t acct_flags, uint32_t \
	  *rid)) \
	X(NTSTATUS, delete_user, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *tmp_ctx, struct samu *sam_acct)) \
	X(NTSTATUS, add_sam_account, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct samu *sampass)) \
	X(NTSTATUS, update_sam_account, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct samu *sampass)) \
	X(NTSTATUS, delete_sam_account, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct samu *username)) \
	X(NTSTATUS, rename_sam_account, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct samu *oldname, const char *newname)) \
	X(NTSTATUS, update_login_attempts, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct samu *sam_acct, bool success)) \
	X(NTSTATUS, getgrsid, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, GROUP_MAP *map, struct dom_sid sid)) \
	X(NTSTATUS, getgrgid, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, GROUP_MAP *map, gid_t gid)) \
	X(NTSTATUS, getgrnam, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, GROUP_MAP *map, const char *name)) \
	X(NTSTATUS, create_dom_group, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, const char *name, uint32_t *rid)) \
	X(NTSTATUS, delete_dom_group, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t rid)) \
	X(NTSTATUS, add_group_mapping_entry, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, GROUP_MAP *map)) \
	X(NTSTATUS, update_group_mapping_entry, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, GROUP_MAP *map)) \
	X(NTSTATUS, delete_group_mapping_entry, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, struct dom_sid sid)) \
	X(NTSTATUS, enum_group_mapping, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *sid, enum lsa_SidType sid_name_use, GROUP_MAP \
	  ***pp_rmap, size_t *p_num_entries, bool unix_only)) \
	X(NTSTATUS, enum_group_members, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, const struct dom_sid *group, uint32_t \
	  **pp_member_rids, size_t *p_num_members)) \
	X(NTSTATUS, enum_group_memberships, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, struct samu *user, struct dom_sid **pp_sids, \
	  gid_t **pp_gids, uint32_t *p_num_groups)) \
	X(NTSTATUS, set_unix_primary_group, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, struct samu *user)) \
	X(NTSTATUS, add_groupmem, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t group_rid, uint32_t member_rid)) \
	X(NTSTATUS, del_groupmem, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t group_rid, uint32_t member_rid)) \
	X(NTSTATUS, create_alias, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char *name, uint32_t *rid)) \
	X(NTSTATUS, delete_alias, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *sid)) \
	X(NTSTATUS, get_aliasinfo, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *sid, struct acct_info *info)) \
	X(NTSTATUS, set_aliasinfo, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *sid, struct acct_info *info)) \
	X(NTSTATUS, add_aliasmem, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *alias, const struct dom_sid *member)) \
	X(NTSTATUS, del_aliasmem, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *alias, const struct dom_sid *member)) \
	X(NTSTATUS, enum_aliasmem, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *alias, TALLOC_CTX *mem_ctx, struct dom_sid \
	  **members, size_t *p_num_members)) \
	X(NTSTATUS, enum_alias_memberships, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, const struct dom_sid *domain_sid, const struct \
	  dom_sid *members, size_t num_members, uint32_t **pp_alias_rids, size_t *p_num_alias_rids)) \
	X(NTSTATUS, lookup_rids, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *domain_sid, int num_rids, uint32_t *rids, \
	  const char **pp_names, enum lsa_SidType *attrs)) \
	X(NTSTATUS, lookup_names, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const struct dom_sid *domain_sid, int num_names, const char \
	  **pp_names, uint32_t *rids, enum lsa_SidType *attrs)) \
	X(NTSTATUS, get_account_policy, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, enum pdb_policy_type type, uint32_t *value)) \
	X(NTSTATUS, set_account_policy, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, enum pdb_policy_type type, uint32_t value)) \
	X(NTSTATUS, get_seq_num, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, time_t *seq_num)) \
	X(bool, search_users, false, \
	  (struct pdb_methods *p, struct pdb_search *search, uint32_t acct_flags)) \
	X(bool, search_groups, false, \
	  (struct pdb_methods *p, struct pdb_search *search)) \
	X(bool, search_aliases, false, \
	  (struct pdb_methods *p, struct pdb_search *search, const struct dom_sid *sid)) \
	X(bool, id_to_sid, false, \
	  (struct pdb_methods *p, struct unixid *id, struct dom_sid *sid)) \
	X(bool, sid_to_id, false, \
	  (struct pdb_methods *p, const struct dom_sid *sid, struct unixid *id)) \
	X(uint32_t, capabilities, 0, \
	  (struct pdb_methods *p)) \
	X(bool, new_rid, false, \
	  (struct pdb_methods *p, uint32_t *rid)) \
	X(bool, get_trusteddom_pw, false, \
	  (struct pdb_methods *p, const char *domain, char** pwd, struct dom_sid *sid, time_t \
	  *pass_last_set_time)) \
	X(NTSTATUS, get_trusteddom_creds, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char *domain, TALLOC_CTX *mem_ctx, struct cli_credentials \
	  **creds)) \
	X(bool, set_trusteddom_pw, false, \
	  (struct pdb_methods *p, const char* domain, const char* pwd, const struct dom_sid *sid)) \
	X(bool, del_trusteddom_pw, false, \
	  (struct pdb_methods *p, const char *domain)) \
	X(NTSTATUS, enum_trusteddoms, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t *num_domains, struct trustdom_info \
	  ***domains)) \
	X(NTSTATUS, get_trusted_domain, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, const char *domain, struct pdb_trusted_domain \
	  **td)) \
	X(NTSTATUS, get_trusted_domain_by_sid, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, struct dom_sid *sid, struct pdb_trusted_domain \
	  **td)) \
	X(NTSTATUS, set_trusted_domain, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char* domain, const struct pdb_trusted_domain *td)) \
	X(NTSTATUS, del_trusted_domain, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char *domain)) \
	X(NTSTATUS, enum_trusted_domains, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t *num_domains, struct \
	  pdb_trusted_domain ***domains)) \
	X(NTSTATUS, get_secret, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, const char *secret_name, DATA_BLOB \
	  *secret_current, NTTIME *secret_current_lastchange, DATA_BLOB *secret_old, NTTIME \
	  *secret_old_lastchange, struct security_descriptor **sd)) \
	X(NTSTATUS, set_secret, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char *secret_name, DATA_BLOB *secret_current, DATA_BLOB \
	  *secret_old, struct security_descriptor *sd)) \
	X(NTSTATUS, delete_secret, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, const char *secret_name)) \
	X(NTSTATUS, enum_upn_suffixes, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, TALLOC_CTX *mem_ctx, uint32_t *num_suffixes, char ***suffixes)) \
	X(NTSTATUS, set_upn_suffixes, NT_STATUS_ACCESS_DENIED, \
	  (struct pdb_methods *p, uint32_t num_suffixes, const char **suffixes)) \
	X(bool, is_responsible_for_our_sam, zxs_runtime_storage_live(), \
	  (struct pdb_methods *p)) \
	X(bool, is_responsible_for_builtin, false, \
	  (struct pdb_methods *p)) \
	X(bool, is_responsible_for_wellknown, false, \
	  (struct pdb_methods *p)) \
	X(bool, is_responsible_for_unix_users, false, \
	  (struct pdb_methods *p)) \
	X(bool, is_responsible_for_unix_groups, false, \
	  (struct pdb_methods *p)) \
	X(bool, is_responsible_for_everything_else, false, \
	  (struct pdb_methods *p))

#define ZXS_PDB_CALLBACK(type, field, answer, params) \
	static type zxs_##field params { return answer; }
ZXS_PDB_FIELDS(ZXS_PDB_CALLBACK)
#undef ZXS_PDB_CALLBACK

#define ZXS_PDB_SLOT(type, field, answer, params) .field = zxs_##field,
static const struct pdb_methods zxs_restricted_methods = {
	.name = "tdbsam",
	ZXS_PDB_FIELDS(ZXS_PDB_SLOT)
	/* No allocated private state or destructor. */
};
#undef ZXS_PDB_SLOT
#undef ZXS_PDB_FIELDS
#endif
