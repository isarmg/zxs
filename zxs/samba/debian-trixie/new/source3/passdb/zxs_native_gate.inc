/* SPDX-License-Identifier: GPL-3.0-or-later
 * Included only in the pinned native tdbsam TU. Online is a read-only
 * consumer, never an initializer/upgrader. Ordinary PDB methods are not
 * a credential interface. Nothing from a USER record leaves this check. */
struct zxs_catalogue {
    const char *login;
    unsigned seen;
    bool bad;
};
static bool exact_key(TDB_DATA key,const char *text)
{
    const size_t n=strlen(text)+1;
    return key.dptr!=NULL && key.dsize==n && memcmp(key.dptr,text,n)==0;
}
static int zxs_check_entry(TDB_DATA k,TDB_DATA v,void *state)
{
    struct zxs_catalogue *c=state;
    unsigned bit=0;
    char user_key[5+64+1];
    if (c->bad || snprintf(user_key,sizeof(user_key),"USER_%s",c->login)!=(int)(5+strlen(c->login))) goto invalid;
    if(exact_key(k,TDBSAM_VERSION_STRING) || exact_key(k,TDBSAM_MINOR_VERSION_STRING)) {
        const bool major=exact_key(k,TDBSAM_VERSION_STRING);
        if(v.dptr==NULL || v.dsize!=4 || IVAL(v.dptr,0)!=(major ? 4U : 0U)) goto invalid;
        bit=major ? 1U : 2U;
    } else if(exact_key(k,"RID_000003e8")) {
        if(!exact_key(v,c->login)) goto invalid;
        bit=4U;
    } else if(exact_key(k,user_key)) {
        if(v.dptr==NULL || v.dsize==0 || v.dsize>1024U*1024 ||
           !zxs_samu_epoch_v4(v.dptr,(uint32_t)v.dsize)) goto invalid;
        bit=8U;
    } else goto invalid;
    if(c->seen & bit) goto invalid;
    c->seen|=bit;return 0;
invalid:
    c->bad=true;return -1; /* traverse can return a count on early exit */
}
static int zxs_linked_entry(struct tdb_context *db,TDB_DATA k,TDB_DATA v,void *state)
{
    return zxs_check_entry(k,v,state);
}
static bool zxs_original_native_db(struct tdb_context *db)
{
    struct stat st;
    return db!=NULL && fstat(tdb_fd(db),&st)==0 && zxs_runtime_native_store(st.st_dev,st.st_ino);
}
bool zxs_native_tdb_audit(const char *path)
{
    const char *login=zxs_runtime_native_login();
    struct tdb_context *db=NULL;
    struct zxs_catalogue physical={.login=login},linked={.login=login};
    char alias[64];int pin;bool ok;
    if(login==NULL || path==NULL || strcmp(path,ZXS_NATIVE_STORE)!=0) return false;
    pin=zxs_runtime_native_pin(alias);
    if(pin<0) return false;
    /* tdb_open receives only the magic link of our original path-only pin.
     * O_RDONLY, no compatibility or recovery permission, no writable mapping.
     * No namespace, PDB name or record is selectable by a client here. */
    db=tdb_open(alias,0,TDB_NOMMAP,O_RDONLY|O_CLOEXEC,0);
    ok=zxs_original_native_db(db) &&
       tdb_check(db,zxs_check_entry,&physical)==0 && !physical.bad && physical.seen==15 &&
       tdb_traverse_read(db,zxs_linked_entry,&linked)==4 && !linked.bad && linked.seen==15 &&
       zxs_original_native_db(db);
    if(db!=NULL && tdb_close(db)!=0) ok=false;
    if(close(pin)!=0) ok=false;
    return ok;
}
