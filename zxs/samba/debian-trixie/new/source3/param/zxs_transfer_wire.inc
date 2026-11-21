/* SPDX-License-Identifier: GPL-3.0-or-later
 * Transport only. It can't issue DELETE or select a new FILE.
 * One accepted FSP owns the command, and the original worker's fresh
 * endpoint belongs only to that request. */
static const uint8_t handoff_ask[16]="ZXS-XFER/ASK-1";
static const uint8_t handoff_result[16]="ZXS-XFER/RESULT1";
static bool transfer_path_kind(uint8_t kind)
{
    return kind==ZXS_XFER_RESTORE||kind==ZXS_XFER_MOVE||kind==ZXS_XFER_COPY_MOVE;
}
static bool transfer_labels(uint8_t kind,const uint8_t *folder,size_t f,const uint8_t *name,size_t n)
{
    if(kind==ZXS_XFER_DELETE||kind==ZXS_XFER_PROBE)return f==0&&n==0;
    if(kind==ZXS_XFER_RENAME)return f==0&&n>0&&n<=ZXS_FILE_NAME&&name!=NULL&&memchr(name,0,n)==NULL&&
        memchr(name,'\\',n)==NULL&&memchr(name,'/',n)==NULL&&memchr(name,':',n)==NULL;
    return transfer_path_kind(kind) && f>0&&f<=ZXS_DIR_NAME && n>0&&n<=ZXS_FILE_NAME &&
        folder!=NULL && name!=NULL && memchr(folder,0,f)==NULL && memchr(name,0,n)==NULL &&
        memchr(folder,'\\',f)==NULL && memchr(name,'\\',n)==NULL;
}
int zxs_xfer_query(int member,uint8_t kind,const uint8_t *folder,size_t f,const uint8_t *name,size_t n,const uint8_t mark[16])
{
    uint8_t b[ZXS_XFER_ASK]={0};int p[2],rc,e;
    const uint8_t *parents=NULL;size_t plen=0;
    if(transfer_path_kind(kind) && name!=NULL && n>0 && n<=ZXS_OD_PATH_MAX){
        const uint8_t *last=NULL;
        for(size_t i=0;i<n;i++)if(name[i]=='\\')last=name+i;
        if(last!=NULL){
            parents=name;plen=(size_t)(last-name);
            n-=plen+1;name=last+1;
            if(plen==0)return bad();
        }
    }
    if(!transfer_labels(kind,folder,f,name,n)||
       (kind==ZXS_XFER_DELETE?(mark==NULL||!cut_id(mark)):mark!=NULL))return bad();
    memcpy(b,handoff_ask,16);b[16]=kind;b[17]=(uint8_t)f;b[18]=(uint8_t)n;b[19]=(plen!=0);
    if(mark!=NULL)memcpy(b+20,mark,16);
    else if(f)memcpy(b+20,folder,f);
    if(n)memcpy(b+20+ZXS_DIR_NAME,name,n);
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=plen!=0?zxs_odir_put_path(p[0],0,parents,plen):0;
    if(rc==0)rc=transfer(member,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);
    e=errno;close(p[1]);
    if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_member_xfer_pending(int member)
{
    uint8_t tag[16];
    ssize_t n=recv(member,tag,sizeof(tag),MSG_PEEK|MSG_TRUNC|MSG_DONTWAIT);
    if(n==ZXS_XFER_ASK&&memcmp(tag,handoff_ask,16)==0)return 1;
    if(n==18+ZXS_DIR_NAME&&memcmp(tag,capq,16)==0)return 2; /* moved FILE capacity on its own endpoint */
    if(n==32)return 0; /* existing exact READ decoder owns the actual tag */
    return bad();
}
int zxs_xfer_take(int member,int worker,uint8_t *kind,uint8_t folder[ZXS_DIR_NAME+1],uint8_t name[ZXS_FILE_NAME+1],uint8_t mark[16],uint16_t *parent_len,uint8_t *parents)
{
    uint8_t b[ZXS_XFER_ASK];int answer=-1;
    if(transfer(member,NULL,b,sizeof(b),-1,&answer,ZXS_DATA_FD,true)!=0)return -1;
    size_t f=b[17],n=b[18];const uint8_t *tail=b+20+ZXS_DIR_NAME;
    if(memcmp(b,handoff_ask,16)!=0||b[19]>1||(b[19]!=0&&!transfer_path_kind(b[16]))||!transfer_labels(b[16],b+20,f,tail,n)||
       same_endpoint_process(answer,worker,false)!=0)goto reject;
    if(b[16]==ZXS_XFER_DELETE&&!cut_id(b+20))goto reject;
    for(size_t i=b[16]==ZXS_XFER_DELETE?16:f;i<ZXS_DIR_NAME;i++)if(b[20+i]!=0)goto reject;
    for(size_t i=n;i<ZXS_FILE_NAME;i++)if(tail[i]!=0)goto reject;
    *kind=b[16];folder[0]=(uint8_t)f;name[0]=(uint8_t)n;
    memset(mark,0,16);if(b[16]==ZXS_XFER_DELETE)memcpy(mark,b+20,16);
    if(f)memcpy(folder+1,b+20,f);
    if(n)memcpy(name+1,tail,n);
    *parent_len=0;
    if(b[19]!=0){
        uint8_t mode;
        if(zxs_odir_take(answer,&mode,parent_len,parents)!=0 || mode!=0)goto reject;
    }
    return answer;
reject:
    close(answer);return bad();
}
int zxs_xfer_reply(int fd,uint8_t status,const uint8_t file[16],const uint8_t op[16])
{
    uint8_t b[ZXS_XFER_RESULT]={0};
    if(status>ZXS_XFER_UNSUPPORTED || (status==0&&(file==NULL||op==NULL||!cut_id(file)||!cut_id(op))) ||
       (status!=0&&(file!=NULL||op!=NULL)))return bad();
    memcpy(b,handoff_result,16);b[16]=status;
    if(status==0){memcpy(b+32,file,16);memcpy(b+48,op,16);}
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_xfer_read(int fd,const uint8_t expected[16],uint8_t *status,uint8_t receipt[16])
{
    uint8_t b[ZXS_XFER_RESULT],zero[15]={0};
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(expected==NULL||status==NULL||memcmp(b,handoff_result,16)!=0||b[16]>ZXS_XFER_UNSUPPORTED||
       memcmp(b+17,zero,sizeof(zero))!=0 ||
       (b[16]==0?(memcmp(b+32,expected,16)!=0||!cut_id(b+48)):(cut_id(b+32)||cut_id(b+48))))return bad();
    *status=b[16];if(receipt!=NULL)memcpy(receipt,b+48,16);return 0;
}
