/* ZXS's fixed read-only adapter, not a SQLite VFS. Only the upstream
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
int zxs_sql_open_read(const char *path,sqlite3 **result) {
    sqlite3 *db=NULL;
    *result=NULL;
    if(path==NULL || path[0]!='/' || zxs_sql_path_limit()<=0) return SQLITE_CANTOPEN;
    int rc=sqlite3_open_v2(path,&db,SQLITE_OPEN_READONLY|SQLITE_OPEN_NOFOLLOW|
                  SQLITE_OPEN_PRIVATECACHE|SQLITE_OPEN_EXRESCODE,"unix");
    if(rc==SQLITE_OK && sqlite3_db_readonly(db,"main")!=1) rc=SQLITE_READONLY;
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
int zxs_sql_named(sqlite3 *db) {
    int moved=1;
    int rc=sqlite3_file_control(db,"main",SQLITE_FCNTL_HAS_MOVED,&moved);
    return rc==SQLITE_OK && moved ? SQLITE_CANTOPEN : rc;
}
/* Derive canonical sqlite_schema exactly as this qualified upstream engine
 * actually stores the project's DDL, including autoindexes and triggers.
 * This private :memory: reference contains ONLY that fixed public DDL, no
 * pool row/credential/filename. It is not attached, persisted or replayed.
 * The real DB is only SELECTed, inside the SAME short transaction as rows. */
int zxs_sql_schema(sqlite3 *actual,const char *ddl) {
    if(sqlite3_get_autocommit(actual)) return SQLITE_MISUSE;
    int rc=one(actual,"PRAGMA main.application_id",SQLITE_INTEGER,0x5a585331,NULL);
    if(rc==SQLITE_OK) rc=one(actual,"PRAGMA main.user_version",SQLITE_INTEGER,1,NULL);
    sqlite3 *model=NULL;sqlite3_stmt *a=NULL,*m=NULL;
    if(rc==SQLITE_OK) rc=sqlite3_open_v2(":memory:",&model,SQLITE_OPEN_READWRITE|SQLITE_OPEN_CREATE|
                 SQLITE_OPEN_MEMORY|SQLITE_OPEN_PRIVATECACHE,"unix");
    if(rc==SQLITE_OK) rc=sqlite3_exec(model,ddl,NULL,NULL,NULL);
    const char *q="SELECT type,name,tbl_name,sql FROM main.sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY";
    if(rc==SQLITE_OK) rc=sqlite3_prepare_v2(actual,q,-1,&a,NULL);
    if(rc==SQLITE_OK) rc=sqlite3_prepare_v2(model,q,-1,&m,NULL);
    while(rc==SQLITE_OK) {
        int ar=sqlite3_step(a),mr=sqlite3_step(m);
        if(ar!=SQLITE_ROW&&ar!=SQLITE_DONE){rc=ar;break;}
        if(mr!=SQLITE_ROW&&mr!=SQLITE_DONE){rc=mr;break;}
        if(ar!=mr){rc=SQLITE_SCHEMA;break;}
        if(ar==SQLITE_DONE)break;
        for(int i=0;i<4;i++){
            int at=sqlite3_column_type(a,i),mt=sqlite3_column_type(m,i);
            if(at!=mt || (at!=SQLITE_NULL&&at!=SQLITE_TEXT)) {rc=SQLITE_SCHEMA;break;}
            if(at==SQLITE_NULL)continue;
            const unsigned char *x=sqlite3_column_text(a,i),*y=sqlite3_column_text(m,i);
            if(!x||!y){rc=SQLITE_NOMEM;break;}
            int n=sqlite3_column_bytes(a,i);
            if(n!=sqlite3_column_bytes(m,i)||memcmp(x,y,(size_t)n)!=0){rc=SQLITE_SCHEMA;break;}
        }
    }
    int x=sqlite3_finalize(a),y=sqlite3_finalize(m),z=model?sqlite3_close(model):SQLITE_OK;
    if(rc==SQLITE_OK) rc=x;
    if(rc==SQLITE_OK) rc=y;
    if(rc==SQLITE_OK) rc=z;
    return rc;
}
