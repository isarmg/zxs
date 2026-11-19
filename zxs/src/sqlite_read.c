/* ZXS's fixed local connection policy, not a SQLite VFS. Only the upstream
 * amalgamation is authorized to open the actual database file. */
#include "sqlite3.h"
#include <stddef.h>
#include <string.h>

int zxs_sql_path_limit(void) {
    if (sqlite3_libversion_number()!=SQLITE_VERSION_NUMBER ||
        strcmp(sqlite3_sourceid(),SQLITE_SOURCE_ID)!=0) return 0;
    sqlite3_vfs *v=sqlite3_vfs_find("unix");
    return v ? v->mxPathname : 0;
}
static int one(sqlite3 *db,const char *sql,int type,int number,const char *text) {
    sqlite3_stmt *s=NULL;
    int rc=sqlite3_prepare_v2(db,sql,-1,&s,NULL);
    if (rc==SQLITE_OK) {
        rc=sqlite3_step(s);
        if (rc==SQLITE_ROW) {
            int match=sqlite3_column_type(s,0)==type;
            if (match && type==SQLITE_INTEGER) match=sqlite3_column_int64(s,0)==number;
            if (match && type==SQLITE_TEXT) {
                const unsigned char *p=sqlite3_column_text(s,0);
                match=p && sqlite3_column_bytes(s,0)==(int)strlen(text) && memcmp(p,text,strlen(text))==0;
            }
            if (!match) rc=SQLITE_MISMATCH;
            else {rc=sqlite3_step(s); if(rc==SQLITE_DONE) rc=SQLITE_OK;else if(rc==SQLITE_ROW) rc=SQLITE_MISMATCH;}
        } else if(rc==SQLITE_DONE) rc=SQLITE_MISMATCH;
    }
    int done=sqlite3_finalize(s); return rc==SQLITE_OK ? done : rc;
}
static int open_existing(const char *path,int writer,sqlite3 **result) {
    sqlite3 *db=NULL;
    *result=NULL;
    if(path==NULL || path[0]!='/' || zxs_sql_path_limit()<=0) return SQLITE_CANTOPEN;
    int rc=sqlite3_open_v2(path,&db,(writer ? SQLITE_OPEN_READWRITE : SQLITE_OPEN_READONLY)|SQLITE_OPEN_NOFOLLOW|
                  SQLITE_OPEN_PRIVATECACHE|SQLITE_OPEN_EXRESCODE,"unix");
    if(rc==SQLITE_OK && sqlite3_db_readonly(db,"main")!=(writer ? 0 : 1)) rc=SQLITE_READONLY;
    if(rc==SQLITE_OK) {
        int n=0;
        rc=sqlite3_db_config(db,SQLITE_DBCONFIG_DEFENSIVE,1,&n);
        if(rc==SQLITE_OK && n!=1) rc=SQLITE_MISMATCH;
    }
    if(rc==SQLITE_OK) {
        sqlite3_limit(db,SQLITE_LIMIT_ATTACHED,0);
        sqlite3_limit(db,SQLITE_LIMIT_LENGTH,1024*1024);
        rc=sqlite3_exec(db,"PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=EXTRA;",NULL,NULL,NULL);
    }
    if(rc==SQLITE_OK) rc=one(db,"PRAGMA foreign_keys",SQLITE_INTEGER,1,NULL);
    if(rc==SQLITE_OK) rc=one(db,"PRAGMA trusted_schema",SQLITE_INTEGER,0,NULL);
    if(rc==SQLITE_OK) rc=one(db,"PRAGMA synchronous",SQLITE_INTEGER,3,NULL);
    if(rc==SQLITE_OK) rc=one(db,"PRAGMA main.journal_mode",SQLITE_TEXT,0,"delete"); /* NEVER set mode here. */
    if(rc!=SQLITE_OK){if(db) sqlite3_close(db);return rc;}
    *result=db;return SQLITE_OK;
}
int zxs_sql_open_read(const char *path,sqlite3 **result) { return open_existing(path,0,result); }
/* No CREATE, no mode conversion and no arbitrary SQL interface. A genuine
 * online operation must recheck its actor and immutable pool in its own
 * BEGIN IMMEDIATE. Pure observations never request this connection. */
int zxs_sql_open_writer(const char *path,sqlite3 **result) { return open_existing(path,1,result); }
int zxs_sql_bind_copy(sqlite3_stmt *stmt,int index,const void *bytes,int count) {
    return sqlite3_bind_blob(stmt,index,bytes,count,SQLITE_TRANSIENT);
}
int zxs_sql_named(sqlite3 *db) {
    int moved=1;
    int rc=sqlite3_file_control(db,"main",SQLITE_FCNTL_HAS_MOVED,&moved);
    return rc==SQLITE_OK && moved ? SQLITE_CANTOPEN : rc;
}
/* Compile the canonical public DDL once per observer, outside any real
 * pool transaction. No user rows, persisted DB, path lookup or ATTACH. The
 * caller retains only the digest of the actual normalized SQLite objects. */
int zxs_sql_template(const char *ddl,sqlite3 **result) {
    sqlite3 *db=NULL;*result=NULL;
    int rc=sqlite3_open_v2(":memory:",&db,SQLITE_OPEN_READWRITE|SQLITE_OPEN_CREATE|
               SQLITE_OPEN_MEMORY|SQLITE_OPEN_PRIVATECACHE,"unix");
    if(rc==SQLITE_OK) rc=sqlite3_exec(db,ddl,NULL,NULL,NULL);
    if(rc!=SQLITE_OK){if(db)sqlite3_close(db);return rc;}
    *result=db;return SQLITE_OK;
}
int zxs_sql_family(sqlite3 *actual) {
    if(sqlite3_get_autocommit(actual))return SQLITE_MISUSE;
    int rc=one(actual,"PRAGMA main.application_id",SQLITE_INTEGER,0x5a585331,NULL);
    return rc==SQLITE_OK ? one(actual,"PRAGMA main.user_version",SQLITE_INTEGER,4,NULL) : rc;
}
