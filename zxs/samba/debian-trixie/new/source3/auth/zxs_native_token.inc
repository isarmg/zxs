/* Only the existing auth3 GENSEC may supply its own single successful
 * attempt and opaque return object; there is no SID/uid-only token factory. */
NTSTATUS zxs_auth3_native_token(TALLOC_CTX *mem_ctx,struct zxs_smb2_attempt *a,
	const struct auth_serversupplied_info *si,const char *username,
	uint32_t flags,struct auth_session_info **out)
{
	TALLOC_CTX *frame=NULL;
	struct auth_user_info_dc *dc=NULL;
	struct auth_session_info *result=NULL;
	struct passwd record,*pw=NULL;
	char scratch[16384];
	union netr_Validation validation;
	struct dom_sid acct,group,ntlm;
	const struct dom_sid *domain;
	unsigned required=0;
	NTSTATUS st=NT_STATUS_LOGON_FAILURE;
	if (out==NULL) return st;
	*out=NULL;
	if (a==NULL || !a->ready || !a->password_verified || si==NULL || a->returned!=si ||
	    a->token!=NULL || !zxs_runtime_client_process() || zxs_smb_try_current(a->fd,a->started_msec)!=0 ||
	    si->guest || si->cached_session_info!=NULL || si->nss_token || si->info3==NULL ||
	    si->info3->base.rid!=1000 || si->info3->base.primary_gid!=513 ||
	    si->info3->base.acct_flags!=ACB_NORMAL || username==NULL ||
	    si->info3->base.account_name.string==NULL || strcmp(username,si->info3->base.account_name.string)!=0 ||
	    si->info3->base.logon_domain.string==NULL || strcmp(si->info3->base.logon_domain.string,"ZXSLOCAL")!=0 ||
	    (flags & AUTH_SESSION_INFO_UNIX_TOKEN)==0) return st;
	domain=get_global_sam_sid();
	if (domain==NULL || si->info3->base.domain_sid==NULL || !dom_sid_equal(domain,si->info3->base.domain_sid) ||
	    !sid_compose(&acct,domain,1000) || !sid_compose(&group,domain,513) ||
	    !dom_sid_parse(SID_NT_NTLM_AUTHENTICATION,&ntlm) ||
	    getpwnam_r("zxs-smb-client",&record,scratch,sizeof(scratch),&pw)!=0 || pw==NULL ||
	    !zxs_fixed_unix_mapping(pw,pw->pw_name,pw->pw_uid,pw->pw_gid)) return st;
	frame=talloc_stackframe();
	validation.sam3=si->info3;
	st=make_user_info_dc_netlogon_validation(frame,username,3,&validation,true,&dc);
	if (!NT_STATUS_IS_OK(st)) goto done;
	if (dc==NULL || dc->num_sids!=2 || dc->sids==NULL ||
	    !dom_sid_equal(&dc->sids[0].sid,&acct) || !dom_sid_equal(&dc->sids[1].sid,&group)) {st=NT_STATUS_INVALID_TOKEN;goto done;}
	st=auth3_user_info_dc_add_hints(dc,pw->pw_uid,pw->pw_gid,
		AUTH3_UNIX_HINT_DONT_EXPAND_UNIX_GROUPS|AUTH3_UNIX_HINT_DONT_TRANSLATE_FROM_SIDS|
		AUTH3_UNIX_HINT_DONT_TRANSLATE_TO_SIDS);
	if (!NT_STATUS_IS_OK(st)) goto done;
	st=auth3_session_info_create(frame,dc,username,AUTH_SESSION_INFO_UNIX_TOKEN|
		AUTH_SESSION_INFO_DEFAULT_GROUPS|AUTH_SESSION_INFO_NTLM|AUTH_SESSION_INFO_SIMPLE_PRIVILEGES,&result,true);
	if (!NT_STATUS_IS_OK(st)) goto done;
	/* Numeric identity AND its reverse NSS spelling must match. Permit
	 * only native ordinary groups, never membership/privilege escalation. */
	if (result==NULL || result->security_token==NULL || result->unix_info==NULL || result->unix_token==NULL ||
	    !zxs_fixed_unix_mapping(pw,result->unix_info->unix_name,result->unix_token->uid,result->unix_token->gid) ||
	    result->security_token->privilege_mask!=0 || result->security_token->rights_mask!=0 ||
	    result->security_token->num_sids<2 || result->security_token->sids==NULL ||
	    !dom_sid_equal(&result->security_token->sids[0],&acct) ||
	    result->unix_token->ngroups!=1 || result->unix_token->groups==NULL ||
	    result->unix_token->groups[0]!=pw->pw_gid) {st=NT_STATUS_INVALID_TOKEN;goto done;}
	for (uint32_t i=0;i<result->security_token->num_sids;i++) {
		const struct dom_sid *v=&result->security_token->sids[i];
		if (dom_sid_equal(v,&acct)) required|=1;
		else if (dom_sid_equal(v,&group)) required|=2;
		else if (dom_sid_equal(v,&global_sid_World)) required|=4;
		else if (dom_sid_equal(v,&global_sid_Network)) required|=8;
		else if (dom_sid_equal(v,&global_sid_Authenticated_Users)) required|=16;
		else if (dom_sid_equal(v,&ntlm)) required|=32;
		else {st=NT_STATUS_INVALID_TOKEN;goto done;}
	}
	if (required!=63 || zxs_smb_try_current(a->fd,a->started_msec)!=0) {st=NT_STATUS_INVALID_TOKEN;goto done;}
	*out=talloc_move(mem_ctx,&result);
done:
	TALLOC_FREE(frame);
	return st;
}
