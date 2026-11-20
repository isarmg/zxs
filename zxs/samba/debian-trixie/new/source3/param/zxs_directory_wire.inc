/* SPDX-License-Identifier: GPL-3.0-or-later
 * Linux-only private socket of the exact authorized TCON. The per-call return
 * pair must belong to its original worker, not another process with its UID.
 * The stable smbd process can create it under the normal request's Unix ID.
 * Nothing here authenticates a principal or looks up a host pathname. */
static const uint8_t dirq[16]="ZXS-DIR/PAGE-1";
static const uint8_t dira[16]="ZXS-DIR/ROWS-1";
static const uint8_t openq[16]="ZXS-FILE/OPEN-1";
static const uint8_t opena[16]="ZXS-FILE/HELD-1";
static int zxs_tree_request(int tree,bool file,unsigned limit,const uint8_t *after,size_t n)
{
    uint8_t b[18+ZXS_DIR_NAME]={0};int p[2],rc,e;
    if(limit>ZXS_DIR_ROWS || n>ZXS_DIR_NAME || (file && (limit!=0 || n==0)) ||
       (n!=0 && ((!file && limit==0) || after==NULL || memchr(after,0,n))))return bad();
    memcpy(b,file?openq:dirq,16);b[16]=(uint8_t)limit;b[17]=(uint8_t)n;
    if(n)memcpy(b+18,after,n);
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=transfer(tree,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);e=errno;close(p[1]);
    if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_dir_query(int tree,unsigned limit,const uint8_t *after,size_t n)
{
    return zxs_tree_request(tree,false,limit,after,n);
}
int zxs_member_query(int tree,const uint8_t *name,size_t n)
{
    return zxs_tree_request(tree,true,0,name,n);
}
int zxs_dir_take(int tree,int worker,uint8_t *limit,uint8_t cursor[223])
{
    uint8_t b[18+ZXS_DIR_NAME];int reply=-1;
    if(transfer(tree,NULL,b,sizeof(b),-1,&reply,ZXS_DATA_FD,true)!=0)return -1;
    size_t n=b[17];
    bool file=memcmp(b,openq,16)==0;
    if((!file && memcmp(b,dirq,16)!=0) || b[16]>ZXS_DIR_ROWS || n>ZXS_DIR_NAME ||
       (file?(b[16]!=0||!n):(b[16]==0 && n)) ||
       (n && memchr(b+18,0,n)!=NULL) || same_endpoint_process(reply,worker,false)!=0)goto invalid;
    for(size_t i=n;i<ZXS_DIR_NAME;i++)if(b[18+i]!=0)goto invalid;
    *limit=file?ZXS_MEMBER_OPEN:b[16];cursor[0]=(uint8_t)n;if(n)memcpy(cursor+1,b+18,n);return reply;
invalid:
    close(reply);return bad();
}
int zxs_dir_reply(int fd,const uint8_t data[ZXS_DIR_PACKET-16])
{
    uint8_t b[ZXS_DIR_PACKET];
    memcpy(b,dira,16);memcpy(b+16,data,sizeof(b)-16);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_dir_result(int fd,unsigned limit,uint8_t data[ZXS_DIR_PACKET-16])
{
    uint8_t b[ZXS_DIR_PACKET];size_t i;
    if(limit>ZXS_DIR_ROWS || data==NULL ||
       transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,dira,16)!=0 || b[16]>limit)return bad();
    for(i=0;i<ZXS_DIR_ROWS;i++){
        const uint8_t *row=b+17+i*ZXS_DIR_SLOT;size_t n=row[0];
        if(i>=b[16]){for(size_t j=0;j<ZXS_DIR_SLOT;j++)if(row[j]!=0)return bad();continue;}
        if(n==0 || n>ZXS_DIR_NAME || memchr(row+1,0,n)!=NULL)return bad();
        for(size_t j=n;j<ZXS_DIR_NAME;j++)if(row[1+j])return bad();
        uint64_t no=0;for(size_t j=0;j<8;j++)no=(no<<8)|row[1+ZXS_DIR_NAME+j];
        if(no<=255 || no==UINT64_MAX || (row[1+ZXS_DIR_NAME+8]&0x80)!=0)return bad();
    }
    memcpy(data,b+16,sizeof(b)-16);return 0;
}

static uint64_t zxs_member_u64(const uint8_t b[8])
{
    uint64_t v=0;for(size_t i=0;i<8;i++)v=(v<<8)|b[i];return v;
}
int zxs_member_reply(int fd,const uint8_t data[ZXS_MEMBER_BYTES])
{
    uint8_t b[16+ZXS_MEMBER_BYTES];memcpy(b,opena,16);memcpy(b+16,data,ZXS_MEMBER_BYTES);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_member_result(int fd,const struct zxs_smb_tree_cut *root,struct zxs_member_info *out)
{
    uint8_t b[16+ZXS_MEMBER_BYTES],zero[16]={0};struct zxs_member_info i={0};
    if(root==NULL || out==NULL || transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,opena,16)!=0 || memcmp(b+16,root->pool,16)!=0 || memcmp(b+32,root->volume,16)!=0 ||
       memcmp(b+48,zero,16)==0 || memcmp(b+64,zero,16)==0)return bad();
    memcpy(i.pool,b+16,16);memcpy(i.volume,b+32,16);memcpy(i.entry,b+48,16);memcpy(i.file,b+64,16);
    i.number=zxs_member_u64(b+80);i.revision=zxs_member_u64(b+88);i.size=zxs_member_u64(b+96);
    if(i.number<=255 || i.number==UINT64_MAX || i.number!=zxs_member_u64(i.file) ||
       i.revision>1 || (!i.revision && i.size) || i.size>INT64_MAX)return bad();
    *out=i;return 0;
}
