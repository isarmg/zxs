/* SPDX-License-Identifier: GPL-3.0-or-later
 * Linux-only private socket of the exact authorized TCON. The per-call return
 * pair must belong to its original worker, not another process with its UID.
 * The stable smbd process can create it under the normal request's Unix ID.
 * Nothing here authenticates a principal or looks up a host pathname. */
static const uint8_t dirq[16]="ZXS-DIR/PAGE-1";
static const uint8_t dira[16]="ZXS-DIR/ROWS-1";
static const uint8_t nsa[16]="ZXS-NS/ROWS-1";
static const uint8_t nsq[16]="ZXS-NS/FOLDER-1";
static const uint8_t capq[16]="ZXS-NS/CAPREQ1";
static const uint8_t capa[16]="ZXS-NS/CAPRES1";
static const uint8_t rounds[16]="ZXS-R/ROUNDS1";
static const uint8_t retainedq[16]="ZXS-R/MEMBER-1";
static const uint8_t openq[16]="ZXS-FILE/OPEN-1";
static const uint8_t editq[16]="ZXS-FILE/EDIT-1";
static const uint8_t opena[16]="ZXS-FILE/HELD-1";
static const uint8_t birthq[16]="ZXS-FILE/MAKE-1";
static const uint8_t offera[16]="ZXS-FILE/OFFER1";
static const uint8_t rejecta[16]="ZXS-FILE/FAIL-1";
static const uint8_t birth_ack_tag[16]="ZXS-FILE/ACK-01";
static const uint8_t birth_done_tag[16]="ZXS-FILE/BORN-1";

/* One bounded relative ordinary path. The SCM_RIGHTS endpoint is accepted
 * only after the normal original worker PIDFD check. Queue its whole exact
 * path before sending the right; there is no client-paced input read. */
static const uint8_t odreq[16]="ZXS-OD/TAKE-01";
static const uint8_t odpath[16]="ZXS-OD/PATH-01";
static const uint8_t odanswer[16]="ZXS-OD/CUT-01";
static const uint8_t odack[16]="ZXS-OD/ACK-01";
static const uint8_t oddone[16]="ZXS-OD/DONE-01";
static const uint8_t odrows[16]="ZXS-OD/ROWS-01";

bool zxs_odir_component(const char *name,size_t n,size_t limit)
{
    if(n==0 || n>limit || name[n-1]=='.' || name[n-1]==' ')return false;
    for(size_t i=0;i<n;i++){
        unsigned char a=name[i];
        if(a<0x20 || a==0x7f || strchr("\\/:*?\"<>|",a)!=NULL)return false;
    }
    return true;
}
bool zxs_odir_relative(const char *path,bool directory)
{
    if(path==NULL || strlen(path)>ZXS_OD_PATH_MAX)return false;
    size_t depth=0;
    for(;;){
        const char *slash=strchr(path,'\\');
        const size_t n=slash==NULL?strlen(path):(size_t)(slash-path);
        if(++depth>256 || !zxs_odir_component(path,n,(slash!=NULL||directory)?ZXS_DIR_NAME:ZXS_FILE_NAME))return false;
        if(slash==NULL)return true;
        path=slash+1;
    }
}
static int zxs_odir_put_path(int fd,uint8_t mode,const uint8_t *name,size_t n)
{
    uint8_t path[ZXS_OD_PATH_REQUEST]={0};
    if(mode>ZXS_OD_SELECT_KIND || n==0 || n>ZXS_OD_PATH_MAX || name==NULL || memchr(name,0,n)!=NULL)return bad();
    memcpy(path,odpath,16);
    path[16]=mode;path[17]=(uint8_t)(n>>8);path[18]=(uint8_t)n;memcpy(path+19,name,n);
    return transfer(fd,path,NULL,sizeof(path),-1,NULL,ZXS_NO_FD,true);
}
int zxs_odir_query(int fd,uint8_t mode,const uint8_t *name,size_t n)
{
    uint8_t outer[18+ZXS_DIR_NAME]={0};int p[2],rc,e;
    memcpy(outer,odreq,16);
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=zxs_odir_put_path(p[0],mode,name,n);
    if(rc==0)rc=transfer(fd,outer,NULL,sizeof(outer),p[1],NULL,ZXS_NO_FD,true);
    e=errno;close(p[1]);if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_odir_take(int fd,uint8_t *mode,uint16_t *width,uint8_t path[ZXS_OD_PATH_MAX])
{
    uint8_t b[ZXS_OD_PATH_REQUEST];size_t n;
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    n=((size_t)b[17]<<8)|b[18];
    if(memcmp(b,odpath,16)!=0 || b[16]>ZXS_OD_SELECT_KIND || n==0 || n>ZXS_OD_PATH_MAX || memchr(b+19,0,n)!=NULL)return bad();
    for(size_t i=n;i<ZXS_OD_PATH_MAX;i++)if(b[19+i])return bad();
    *mode=b[16];*width=(uint16_t)n;memcpy(path,b+19,n);return 0;
}
int zxs_odir_reply(int fd,uint8_t kind,const uint8_t *base,const uint8_t *id,const uint8_t *op)
{
    uint8_t b[ZXS_OD_PATH_RESULT]={0};
    memcpy(b,odanswer,16);b[16]=kind;
    if(kind<=1 && base!=NULL && id!=NULL && op!=NULL && cut_id(id) && cut_id(op)){
        memcpy(b+17,base,ZXS_SMB_TREE_CUT_BYTES);
        memcpy(b+17+ZXS_SMB_TREE_CUT_BYTES,id,16);memcpy(b+33+ZXS_SMB_TREE_CUT_BYTES,op,16);
    }else if(kind<2 || kind>ZXS_OD_FILE_KIND || base!=NULL || id!=NULL || op!=NULL)return bad();
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
static bool same_ordinary_cut(const struct zxs_smb_tree_cut *a,const struct zxs_smb_tree_cut *b)
{
    return a->mode==b->mode && a->revision==b->revision && a->volume_ordinal==b->volume_ordinal &&
        a->volume_ordinal>=2 && a->volume_ordinal<64 &&
        memcmp(a->pool,b->pool,16)==0 && memcmp(a->volume,b->volume,16)==0 &&
        memcmp(a->share,b->share,16)==0 && memcmp(a->setting,b->setting,16)==0;
}
int zxs_odir_read(int fd,const struct zxs_smb_tree_cut *parent,struct zxs_smb_tree_cut *out)
{
    uint8_t b[ZXS_OD_PATH_RESULT];uint64_t revision=0;
    struct zxs_smb_tree_cut child={.root_fd=-1,.parent_fd=-1};
    if(parent==NULL || out==NULL || !cut_id(parent->volume) || cut_id(parent->directory) ||
       transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,odanswer,16)!=0 || b[16]>ZXS_OD_FILE_KIND)return bad();
    if(b[16]>=2){
        for(size_t i=17;i<sizeof(b);i++)if(b[i])return bad();
        return b[16];
    }
    const uint8_t *base=b+17;
    for(size_t i=48;i<56;i++)revision=(revision<<8)|base[i];
    memcpy(child.share,base,16);memcpy(child.volume,base+16,16);memcpy(child.setting,base+32,16);
    child.revision=revision;child.mode=base[56];memcpy(child.pool,base+57,16);child.volume_ordinal=base[73];
    memcpy(child.directory,base+ZXS_SMB_TREE_CUT_BYTES,16);
    memcpy(child.directory_op,base+ZXS_SMB_TREE_CUT_BYTES+16,16);
    if(!cut_id(child.directory) || !cut_id(child.directory_op) || !same_ordinary_cut(parent,&child) ||
       (b[16]==1 && child.mode!=2))return bad();
    *out=child;return b[16];
}
int zxs_odir_own_parent(struct zxs_smb_tree_cut *child,struct zxs_smb_tree_cut *parent)
{
    if(child==NULL || parent==NULL || child->root_fd<0 || parent->root_fd<0 ||
       child->root_fd==parent->root_fd || child->owns_parent || parent->owns_parent ||
       !cut_id(child->directory) || cut_id(parent->directory) || !same_ordinary_cut(child,parent) ||
       zxs_smb_ctl_live(child->root_fd)!=0 || zxs_smb_ctl_live(parent->root_fd)!=0)return bad();
    child->owns_parent=true;child->parent_fd=parent->root_fd;parent->root_fd=-1;return 0;
}
static int odir_boundary(int fd,const uint8_t tag[16],const uint8_t id[16],const uint8_t op[16],bool send,bool wait)
{
    uint8_t b[ZXS_OD_PATH_ACK];
    if(!cut_id(id)||!cut_id(op))return bad();
    if(send){
        memcpy(b,tag,16);memcpy(b+16,id,16);memcpy(b+32,op,16);
        return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
    }
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,!wait)!=0)return -1;
    return memcmp(b,tag,16)==0 && memcmp(b+16,id,16)==0 && memcmp(b+32,op,16)==0?0:bad();
}
int zxs_odir_ack(int fd,const uint8_t id[16],const uint8_t op[16]){return odir_boundary(fd,odack,id,op,true,false);}
int zxs_odir_take_ack(int fd,const uint8_t id[16],const uint8_t op[16]){return odir_boundary(fd,odack,id,op,false,true);}
int zxs_odir_done(int fd,const uint8_t id[16],const uint8_t op[16]){return odir_boundary(fd,oddone,id,op,true,false);}
int zxs_odir_read_done(int fd,const uint8_t id[16],const uint8_t op[16]){return odir_boundary(fd,oddone,id,op,false,false);}


bool zxs_delete_hex(const uint8_t *name,size_t n)
{
    bool nonzero=false,nonmax=false;
    if(name==NULL||n!=32)return false;
    for(size_t i=0;i<n;i++){
        uint8_t x=name[i];
        if(!((x>='0'&&x<='9')||(x>='a'&&x<='f')))return false;
        nonzero|=x!='0';nonmax|=x!='f';
    }
    return nonzero&&nonmax;
}
static bool zxs_retained_path(const uint8_t *name,size_t n)
{
    return name!=NULL && n>=34 && n<=ZXS_DIR_NAME &&
        zxs_delete_hex(name,32) && name[32]=='\\' && memchr(name+33,'\\',n-33)==NULL;
}
static int zxs_tree_request(int tree,const uint8_t tag[16],unsigned limit,const uint8_t *after,size_t n)
{
    uint8_t b[18+ZXS_DIR_NAME]={0};int p[2],rc,e;
    const bool capacity=tag==capq,file=tag!=dirq&&!capacity;
    if(limit>ZXS_DIR_ROWS || n>ZXS_DIR_NAME || (capacity && (limit>1 || n!=0)) ||
       (file && (n==0 || (tag==nsq?limit>2:tag==retainedq?(limit>1||!zxs_retained_path(after,n)):
                    (n>ZXS_FILE_NAME || (tag==birthq?limit>3:limit!=0))))) ||
       (n!=0 && ((!file && limit==0) || after==NULL || memchr(after,0,n))))return bad();
    memcpy(b,tag,16);b[16]=(uint8_t)limit;b[17]=(uint8_t)n;
    if(n)memcpy(b+18,after,n);
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=transfer(tree,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);e=errno;close(p[1]);
    if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_dir_query(int tree,unsigned limit,const uint8_t *after,size_t n)
{
    return zxs_tree_request(tree,dirq,limit,after,n);
}
int zxs_ns_capacity_query(int tree,bool may_write)
{
    return zxs_tree_request(tree,capq,may_write?1:0,NULL,0);
}
int zxs_r_member_query(int tree,const uint8_t *name,size_t n,bool share_delete)
{
    return zxs_tree_request(tree,retainedq,share_delete?1:0,name,n);
}
int zxs_member_query(int tree,const uint8_t *name,size_t n)
{
    return zxs_tree_request(tree,openq,0,name,n);
}
int zxs_member_write_query(int tree,const uint8_t *name,size_t n)
{
    return zxs_tree_request(tree,editq,0,name,n);
}
int zxs_ns_folder_query(int tree,const uint8_t *name,size_t n,unsigned access)
{
    return zxs_tree_request(tree,nsq,access,name,n);
}
int zxs_birth_query(int tree,const uint8_t *name,size_t n,unsigned flags)
{
    return zxs_tree_request(tree,birthq,flags,name,n);
}
int zxs_dir_take(int tree,int worker,uint8_t *limit,uint8_t cursor[ZXS_DIR_NAME+1])
{
    uint8_t b[18+ZXS_DIR_NAME];int reply=-1;
    if(transfer(tree,NULL,b,sizeof(b),-1,&reply,ZXS_DATA_FD,true)!=0)return -1;
    size_t n=b[17];
    bool edit=memcmp(b,editq,16)==0;
    bool file=edit || memcmp(b,openq,16)==0;
    bool folder=memcmp(b,nsq,16)==0,birth=memcmp(b,birthq,16)==0;
    bool retained=memcmp(b,retainedq,16)==0,path=memcmp(b,odreq,16)==0;
    bool capacity=memcmp(b,capq,16)==0;
    if((!file && !folder && !birth && !retained && !path && !capacity && memcmp(b,dirq,16)!=0) ||
       b[16]>ZXS_DIR_ROWS || n>ZXS_DIR_NAME ||
       (capacity?(b[16]>1||n!=0):path?(b[16]!=0||n!=0):folder?(b[16]>2||!n):retained?(b[16]>1||!zxs_retained_path(b+18,n)):
          (file||birth)?(b[16]>(birth?3:0)||!n||n>ZXS_FILE_NAME):(b[16]==0 && n)) ||
       (n && memchr(b+18,0,n)!=NULL) || same_endpoint_process(reply,worker,false)!=0)goto invalid;
    for(size_t i=n;i<ZXS_DIR_NAME;i++)if(b[18+i]!=0)goto invalid;
    if(capacity){*limit=b[16]?ZXS_NS_CAPACITY_RW:ZXS_NS_CAPACITY_RO;cursor[0]=0;return reply;}
    *limit=path?ZXS_OD_PATH:retained?(ZXS_RETAINED_OPEN+b[16]):birth?(ZXS_FILE_BIRTH+b[16]):folder?(b[16]==0?ZXS_NS_OPEN:(b[16]==1?ZXS_NS_WRITE:ZXS_NS_MAXIMUM)):edit?ZXS_MEMBER_WRITE_OPEN:(file?ZXS_MEMBER_OPEN:b[16]);cursor[0]=(uint8_t)n;if(n)memcpy(cursor+1,b+18,n);return reply;
invalid:
    close(reply);return bad();
}
static int zxs_dir_send(int fd,const uint8_t data[ZXS_DIR_PACKET-16],const uint8_t tag[16])
{
    uint8_t b[ZXS_DIR_PACKET];
    memcpy(b,tag,16);memcpy(b+16,data,sizeof(b)-16);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_dir_reply(int fd,const uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_send(fd,data,dira);}
int zxs_odir_rows(int fd,const uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_send(fd,data,odrows);}
int zxs_ns_dir_reply(int fd,const uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_send(fd,data,nsa);}
int zxs_r_round_rows(int fd,const uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_send(fd,data,rounds);}
static int zxs_dir_receive(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16],const uint8_t tag[16])

{
    uint8_t b[ZXS_DIR_PACKET];size_t i;bool recycle_seen=false;
    if(limit>ZXS_DIR_ROWS || data==NULL ||
       transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,tag,16)!=0 || b[16]>limit)return bad();
    const bool ns=tag==nsa, retained=tag==rounds, ordinary=tag==odrows;
    for(i=0;i<ZXS_DIR_ROWS;i++){
        const uint8_t *row=b+17+i*ZXS_DIR_SLOT;size_t n=row[0];
        if(i>=b[16]){for(size_t j=0;j<ZXS_DIR_SLOT;j++)if(row[j]!=0)return bad();continue;}
        const uint8_t *len=row+1+ZXS_DIR_NAME+8;
        bool directory=ordinary && (len[0]&0x80)!=0;
        if(n==0 || n>((ns||directory)?ZXS_DIR_NAME:ZXS_FILE_NAME) || memchr(row+1,0,n)!=NULL)return bad();
        for(size_t j=n;j<ZXS_DIR_NAME;j++)if(row[1+j])return bad();
        uint64_t no=0;for(size_t j=0;j<8;j++)no=(no<<8)|row[1+ZXS_DIR_NAME+j];
        if(ns){
            if(no<2||no>64||(no==2&&recycle_seen)||memcmp(row+1+ZXS_DIR_NAME+8,"\0\0\0\0\0\0\0\0",8)!=0)return bad();
            recycle_seen |= no==2;
        }else if(retained){
            if(!zxs_delete_hex(row+1,n) || no!=UINT64_MAX ||
               memcmp(row+1+ZXS_DIR_NAME+8,"\0\0\0\0\0\0\0\0",8)!=0)return bad();
        }else if(directory){
            const uint8_t marker[8]={0x80};
            if(no!=UINT64_MAX || memcmp(len,marker,8)!=0)return bad();
        }else if(no<=255 || no==UINT64_MAX || (len[0]&0x80)!=0)return bad();
    }
    memcpy(data,b+16,sizeof(b)-16);return 0;
}

int zxs_odir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_receive(fd,limit,data,odrows);}
int zxs_dir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_receive(fd,limit,data,dira);}
int zxs_ns_dir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_receive(fd,limit,data,nsa);}
int zxs_r_round_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16]){return zxs_dir_receive(fd,limit,data,rounds);}

static uint64_t zxs_member_u64(const uint8_t b[8])
{
    uint64_t v=0;for(size_t i=0;i<8;i++)v=(v<<8)|b[i];return v;
}
static bool zxs_capacity_shape(uint8_t status,uint64_t total,uint64_t available,uint64_t free_bytes)
{
    return status==ZXS_NS_CAP_READY?(total>=4096 && available<=free_bytes && free_bytes<=total):
        ((status==ZXS_NS_CAP_NO_MEDIA || status==ZXS_NS_CAP_BUSY) && total==0 && available==0 && free_bytes==0);
}
int zxs_ns_capacity_reply(int fd,uint8_t status,uint64_t total,uint64_t available,uint64_t free_bytes)
{
    uint8_t wire[48]={0};
    const uint64_t fields[]={total,available,free_bytes};
    if(!zxs_capacity_shape(status,total,available,free_bytes))return bad();
    memcpy(wire,capa,16);wire[16]=status;
    for(size_t i=0;i<3;i++)for(size_t j=0;j<8;j++){wire[24+8*i+j]=(uint8_t)(fields[i]>>(8*(7-j)));}
    return transfer(fd,wire,NULL,sizeof(wire),-1,NULL,ZXS_NO_FD,true);
}
int zxs_ns_capacity_read(int fd,struct zxs_ns_space *out)
{
    uint8_t wire[48];
    if(out==NULL || transfer(fd,NULL,wire,sizeof(wire),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(wire,capa,16)!=0)return bad();
    for(size_t i=17;i<24;i++)if(wire[i]!=0)return bad();
    struct zxs_ns_space got={.total=zxs_member_u64(wire+24),.available=zxs_member_u64(wire+32),
        .free=zxs_member_u64(wire+40),.status=wire[16]};
    if(!zxs_capacity_shape(got.status,got.total,got.available,got.free))return bad();
    *out=got;return 0;
}
int zxs_member_reply(int fd,const uint8_t data[ZXS_MEMBER_BYTES])
{
    uint8_t b[16+ZXS_MEMBER_BYTES];memcpy(b,opena,16);memcpy(b+16,data,ZXS_MEMBER_BYTES);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_birth_reply(int fd,const uint8_t data[ZXS_MEMBER_BYTES],uint8_t kind)
{
    uint8_t b[16+ZXS_MEMBER_BYTES]={0};
    if(kind==ZXS_BIRTH_ACCEPT && data!=NULL){memcpy(b,offera,16);memcpy(b+16,data,ZXS_MEMBER_BYTES);}
    else if(kind>=ZXS_BIRTH_OCCUPIED && kind<=ZXS_BIRTH_FULL && data==NULL){
        memcpy(b,rejecta,16);b[16]=kind;
    }else{return bad();}
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_birth_result(int fd,const struct zxs_smb_tree_cut *root,struct zxs_member_info *out)
{
    uint8_t b[16+ZXS_MEMBER_BYTES],zero[16]={0};struct zxs_member_info i={0};
    if(root==NULL || out==NULL || transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,rejecta,16)==0){
        if(b[16]<ZXS_BIRTH_OCCUPIED||b[16]>ZXS_BIRTH_FULL)return bad();
        for(size_t j=17;j<sizeof(b);j++)if(b[j]!=0)return bad();
        return b[16];
    }
    const bool offer=memcmp(b,offera,16)==0;
    if((!offer && memcmp(b,opena,16)!=0) || memcmp(b+16,root->pool,16)!=0 || memcmp(b+32,root->volume,16)!=0 ||
       memcmp(b+48,zero,16)==0 || memcmp(b+64,zero,16)==0)return bad();
    memcpy(i.pool,b+16,16);memcpy(i.volume,b+32,16);memcpy(i.entry,b+48,16);memcpy(i.file,b+64,16);
    i.number=zxs_member_u64(b+80);i.revision=zxs_member_u64(b+88);i.size=zxs_member_u64(b+96);
    if(i.number<=255 || i.number==UINT64_MAX || i.number!=zxs_member_u64(i.file) ||
       i.revision>INT64_MAX || (!i.revision && i.size) || i.size>INT64_MAX)return bad();
    if(offer && (i.size!=0 || i.revision!=0))return bad();
    *out=i;return offer?ZXS_BIRTH_ACCEPT:0;
}
int zxs_member_result(int fd,const struct zxs_smb_tree_cut *root,struct zxs_member_info *out)
{
    struct zxs_member_info i;
    if(out==NULL || zxs_birth_result(fd,root,&i)!=0)return bad();
    *out=i;return 0;
}
static int birth_send_cut(int fd,const uint8_t tag[16],const uint8_t file[16])
{
    uint8_t b[32];memcpy(b,tag,16);memcpy(b+16,file,16);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
static int birth_recv_cut(int fd,const uint8_t tag[16],const uint8_t file[16],bool nowait)
{
    uint8_t b[32];
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,nowait)!=0)return -1;
    return memcmp(b,tag,16)==0 && memcmp(b+16,file,16)==0?0:bad();
}
int zxs_birth_ack(int fd,const uint8_t file[16]){return birth_send_cut(fd,birth_ack_tag,file);}
int zxs_birth_take_ack(int fd,const uint8_t file[16]){return birth_recv_cut(fd,birth_ack_tag,file,false);}
int zxs_birth_done(int fd,const uint8_t file[16]){return birth_send_cut(fd,birth_done_tag,file);}
int zxs_birth_read_done(int fd,const uint8_t file[16]){return birth_recv_cut(fd,birth_done_tag,file,true);}
