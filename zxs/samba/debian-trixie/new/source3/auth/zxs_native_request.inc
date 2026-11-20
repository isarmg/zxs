/* SPDX-License-Identifier: GPL-3.0-or-later
 * Included only in the native auth3 object. The sole input is the existing
 * accepted GENSEC request. This is not a passdb getter, external checker or
 * IPC protocol. The original password representation never leaves this TU's
 * Samba account/checking path; only standard NTLMSSP completion moves on. */
struct zxs_native_lookup {
	struct samu *sam;
	const char *key;
	bool found;
};
static int zxs_native_read_user(TDB_DATA key,TDB_DATA value,void *p)
{
	struct zxs_native_lookup *r=p;
	if (r->found || key.dsize!=strlen(r->key)+1 || key.dptr==NULL ||
	    memcmp(key.dptr,r->key,key.dsize)!=0 || value.dptr==NULL ||
	    value.dsize==0 || value.dsize>1024U*1024 ||
	    !init_samu_from_buffer(r->sam,4,value.dptr,(uint32_t)value.dsize)) {
		return -1;
	}
	r->found=true;
	return 0;
}
static bool zxs_auth_store_same(struct tdb_context *db)
{
	struct stat st;
	return db!=NULL && fstat(tdb_fd(db),&st)==0 &&
		zxs_runtime_native_store(st.st_dev,st.st_ino);
}
static bool zxs_get_current_for_request(struct samu *sam,const char *selected)
{
	char path[64],key[5+64+1];
	struct zxs_native_lookup read={.sam=sam,.key=key};
	struct tdb_context *db=NULL;
	TDB_DATA k;
	int pin;
	bool ok=false;
	if (snprintf(key,sizeof(key),"USER_%s",selected)!=(int)(5+strlen(selected))) return false;
	k.dptr=(uint8_t *)key;k.dsize=strlen(key)+1;
	pin=zxs_runtime_native_pin(path);
	if (pin<0) return false;
	db=tdb_open(path,0,TDB_NOMMAP,O_RDONLY|O_CLOEXEC,0);
	if (zxs_auth_store_same(db) && tdb_parse_record(db,k,zxs_native_read_user,&read)==0 &&
	    read.found && zxs_auth_store_same(db)) ok=true;
	if (db!=NULL && tdb_close(db)!=0) ok=false;
	if (close(pin)!=0) ok=false;
	return ok;
}

/* ctx and its challenge are the very objects retained by the smbd GENSEC,
 * not caller-selected strings or challenge bytes. An attempt is consumed
 * before lookup even on failure. No pathname is provided by a client. */
NTSTATUS zxs_auth3_native_request(TALLOC_CTX *mem_ctx,struct auth_context *ctx,
	const struct auth_usersupplied_info *user,struct auth_serversupplied_info **out)
{
	struct zxs_smb2_attempt *a=ctx!=NULL ? ctx->zxs_attempt : NULL;
	const char *selected=zxs_runtime_native_login();
	struct samu *sam=NULL;
	struct auth_serversupplied_info *si=NULL;
	struct netr_SamBaseInfo *base;
	const struct dom_sid *domain,*account;
	uint32_t rid=0;
	bool read_ok=false;
	NTSTATUS status=NT_STATUS_LOGON_FAILURE;
	if (out==NULL) return status;
	*out=NULL;
	if (a==NULL || !a->ready || a->password_started || !zxs_runtime_client_process() ||
	    zxs_smb_try_current(a->fd,a->started_msec)!=0) return status;
	a->password_started=true;
	if (selected==NULL || selected[0]=='\0' || user==NULL ||
	    user->password_state!=AUTH_PASSWORD_RESPONSE ||
	    user->client.account_name==NULL || strcmp(user->client.account_name,selected)!=0 ||
	    user->client.domain_name==NULL || strcmp(user->client.domain_name,"ZXSLOCAL")!=0 ||
	    user->password.response.lanman.length!=0 || user->password.response.nt.data==NULL ||
	    user->password.response.nt.length<48 || user->password.response.nt.length>UINT16_MAX ||
	    ctx->challenge.length!=sizeof(a->challenge) || ctx->challenge.data==NULL ||
	    memcmp(ctx->challenge.data,a->challenge,sizeof(a->challenge))!=0 ||
	    lp_ntlm_auth()!=NTLM_AUTH_NTLMV2_ONLY || lp_lanman_auth() || lp_null_passwords()) return status;

	sam=samu_new(mem_ctx);
	if (sam==NULL) return NT_STATUS_NO_MEMORY;
	/* No tevent yield in the root region. It only opens this instance's
	 * protected original, does the native bounded decode, then closes. */
	become_root();
	zxs_sam_require_current();
	read_ok=zxs_get_current_for_request(sam,selected);
	unbecome_root();
	if (!read_ok || !zxs_runtime_client_process() ||
	    zxs_smb_try_current(a->fd,a->started_msec)!=0) goto done;
	domain=get_global_sam_sid();
	account=pdb_get_user_sid(sam);
	if (domain==NULL || account==NULL || !sid_peek_check_rid(domain,account,&rid) || rid!=1000 ||
	    strcmp(pdb_get_username(sam),selected)!=0 || strcmp(pdb_get_domain(sam),"ZXSLOCAL")!=0 ||
	    pdb_get_acct_ctrl(sam)!=ACB_NORMAL || pdb_get_lanman_passwd(sam)!=NULL ||
	    pdb_get_nt_passwd(sam)==NULL || pdb_get_pass_last_set_time(sam)==0) goto done;

	si=make_server_info(mem_ctx);
	if (si==NULL) {status=NT_STATUS_NO_MEMORY;goto done;}
	status=sam_password_ok(si,selected,ACB_NORMAL,&ctx->challenge,NULL,
			       pdb_get_nt_passwd(sam),user,&si->session_key,&si->lm_session_key);
	if (!NT_STATUS_IS_OK(status)) goto done;
	if (si->session_key.length!=16 || si->session_key.data==NULL || si->lm_session_key.length!=0 ||
	    zxs_smb_try_current(a->fd,a->started_msec)!=0) {status=NT_STATUS_LOGON_FAILURE;goto done;}
	/* Only materialize public identity after the ORIGINAL native password
	 * routine succeeded. Group RID 513 was checked in the very same SAMU. */
	si->info3=talloc_zero(si,struct netr_SamInfo3);
	if (si->info3==NULL) {status=NT_STATUS_NO_MEMORY;goto done;}
	base=&si->info3->base;
	base->account_name.string=talloc_strdup(si->info3,selected);
	base->logon_domain.string=talloc_strdup(si->info3,"ZXSLOCAL");
	base->logon_server.string=talloc_strdup(si->info3,"ZXSLOCAL");
	base->domain_sid=dom_sid_dup(si->info3,domain);
	if (base->account_name.string==NULL || base->logon_domain.string==NULL ||
	    base->logon_server.string==NULL || base->domain_sid==NULL) {status=NT_STATUS_NO_MEMORY;goto done;}
	base->rid=rid;base->primary_gid=513;base->acct_flags=ACB_NORMAL;
	unix_to_nt_time(&base->last_password_change,pdb_get_pass_last_set_time(sam));
	unix_to_nt_time(&base->logoff_time,pdb_get_logoff_time(sam));
	unix_to_nt_time(&base->kickoff_time,pdb_get_kickoff_time(sam));
	a->password_verified=true; *out=si; si=NULL;
done:
	if (si!=NULL) {data_blob_clear_free(&si->session_key);data_blob_clear_free(&si->lm_session_key);}
	TALLOC_FREE(si);TALLOC_FREE(sam); /* original samu destructor clears the credential */
	return status;
}
