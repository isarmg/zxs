/* SPDX-License-Identifier: Apache-2.0 */
#define _GNU_SOURCE 1
#include "zxs_smb_control.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/magic.h>
#include <sys/vfs.h>
#include <poll.h>
#include <stdbool.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

/* SMB share names are case-insensitive. Folder spelling remains byte-exact. */
int zxs_smb_namespace_name(const char *name)
{
    return name!=NULL && strnlen(name,4)==3 &&
        (name[0]=='Z'||name[0]=='z') && (name[1]=='X'||name[1]=='x') &&
        (name[2]=='S'||name[2]=='s');
}

/* Exact UAPI of our pinned Debian amd64 / Linux 6.12. */
#ifndef SO_PASSPIDFD
#define SO_PASSPIDFD 76
#endif
#ifndef SCM_PIDFD
#define SCM_PIDFD 4
#endif
static int bad(void) { errno=EPROTO; return -1; }
static bool unnamed(int fd, bool peer)
{
    struct sockaddr_un a={0};socklen_t n=sizeof(a);
    int rc=peer?getpeername(fd,(struct sockaddr *)&a,&n):getsockname(fd,(struct sockaddr *)&a,&n);
    return rc==0 && n==offsetof(struct sockaddr_un,sun_path) && a.sun_family==AF_UNIX;
}
static bool opt(int fd,int k,int expected)
{
    int v=-1;socklen_t n=sizeof(v);
    return getsockopt(fd,SOL_SOCKET,k,&v,&n)==0 && n==sizeof(v) && v==expected;
}
static int claim_socket_kind(int fd,bool root_creator,int kind)
{
    int flags=fcntl(fd,F_GETFD);struct ucred creator={0};socklen_t n=sizeof(creator);
    /* With a socketpair SO_PEERCRED identifies the creator, NOT the exec'd
     * peer. Identity and sole delivery of opposite endpoints are the trusted
     * launcher's job; there is no connectable name or UID-only guest path. */
    int open_flags=fcntl(fd,F_GETFL);
    if(flags<0 || open_flags<0 || (open_flags&O_NONBLOCK)!=0 || !opt(fd,SO_TYPE,kind) || !opt(fd,SO_ACCEPTCONN,0) || !opt(fd,SO_PASSCRED,0) || !opt(fd,SO_PASSPIDFD,0) ||
       !unnamed(fd,false) || !unnamed(fd,true) ||
       getsockopt(fd,SOL_SOCKET,SO_PEERCRED,&creator,&n)<0 || n!=sizeof(creator) ||
       (root_creator && creator.uid!=0)) {return bad();}
    return fcntl(fd,F_SETFD,flags|FD_CLOEXEC);
}
static int claim_socket(int fd,bool root){return claim_socket_kind(fd,root,SOCK_SEQPACKET);}
int zxs_smb_ctl_claim(int fd){return claim_socket(fd,true);}
static int millis(int64_t *out)
{
    struct timespec t;
    if(clock_gettime(CLOCK_MONOTONIC,&t)<0)return -1;
    *out=(int64_t)t.tv_sec*1000+t.tv_nsec/1000000;return 0;
}
/* Only the exact first request can carry one newly-created kernel pipe
 * read end. Never accept a pathname, regular file, directory or writable fd. */
static bool life_reader(int fd)
{
    struct stat st;struct statfs fs;int f=fcntl(fd,F_GETFL),d=fcntl(fd,F_GETFD);
    return f>=0 && d>=0 && (f&O_ACCMODE)==O_RDONLY && (f&O_PATH)==0 &&
           (f&O_NONBLOCK)!=0 && (d&FD_CLOEXEC)!=0 &&
           fstat(fd,&st)==0 && S_ISFIFO(st.st_mode) && fstatfs(fd,&fs)==0 &&
           (unsigned long)fs.f_type==PIPEFS_MAGIC;
}
enum inbound { ZXS_NO_FD, ZXS_LIFE_FD, ZXS_TREE_FD, ZXS_DATA_FD, ZXS_READ_STREAM };
static int transfer(int fd,const uint8_t *sendbuf,uint8_t *recvbuf,size_t len,int offer,int *take,enum inbound kind,bool nowait)
{
    int64_t now,stop;bool sending=sendbuf!=NULL;
    if(len!=ZXS_SMB_BIND_LEN && len!=ZXS_SMB_READY_LEN && len!=ZXS_PLAN_OFFER && len!=ZXS_PLAN_ACK && len!=16 && len!=24 && len!=32 && len!=24+ZXS_SMB_CUT_BYTES && len!=16+ZXS_SMB_TREE_CUT_BYTES && len!=18+ZXS_DIR_NAME && len!=ZXS_DIR_PACKET && len!=16+ZXS_MEMBER_BYTES)return bad();
    if(millis(&now)<0)return -1;
    stop=now+10000;
    for(;;) {
        struct pollfd pollfd={.fd=fd,.events=sending?POLLOUT:POLLIN};
        if(millis(&now)<0)return -1;
        if(now>=stop){errno=ETIMEDOUT;return -1;}
        int r=poll(&pollfd,1,nowait?0:(int)(stop-now));
        if(r<0 && errno==EINTR && !nowait)continue;
        if(r<0)return -1;
        if(r==0){errno=nowait?EAGAIN:ETIMEDOUT;return -1;}
        if((pollfd.revents&pollfd.events)==0)return bad();
        ssize_t got;int received=-1;
        if(sending && offer>=0){
            struct iovec iov={.iov_base=(void *)sendbuf,.iov_len=len};
            union { struct cmsghdr align; unsigned char buf[CMSG_SPACE(sizeof(int))]; } ctrl={0};
            struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1,.msg_control=ctrl.buf,.msg_controllen=sizeof(ctrl.buf)};
            struct cmsghdr *c=CMSG_FIRSTHDR(&msg);
            c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_RIGHTS;c->cmsg_len=CMSG_LEN(sizeof(int));
            memcpy(CMSG_DATA(c),&offer,sizeof(offer));
            got=sendmsg(fd,&msg,MSG_DONTWAIT|MSG_NOSIGNAL);
        }else if(sending){got=send(fd,sendbuf,len,MSG_DONTWAIT|MSG_NOSIGNAL);}
        else{
            struct iovec iov={.iov_base=recvbuf,.iov_len=len};
            struct msghdr msg={.msg_iov=&iov,.msg_iovlen=1};
            union { struct cmsghdr align; unsigned char buf[CMSG_SPACE(4*sizeof(int))]; } ctrl={0};
            if(take!=NULL){msg.msg_control=ctrl.buf;msg.msg_controllen=sizeof(ctrl.buf);}
            got=recvmsg(fd,&msg,MSG_DONTWAIT|MSG_TRUNC|MSG_CMSG_CLOEXEC);
            if(got>=0){
                struct cmsghdr *c;bool extra=(msg.msg_flags&(MSG_CTRUNC|MSG_TRUNC))!=0;
                for(c=CMSG_FIRSTHDR(&msg);c!=NULL;c=CMSG_NXTHDR(&msg,c)){
                    if(c->cmsg_level==SOL_SOCKET && c->cmsg_type==SCM_PIDFD && c->cmsg_len==CMSG_LEN(sizeof(int))){
                        int v;memcpy(&v,CMSG_DATA(c),sizeof(v));if(v>=0)close(v);extra=true;continue;
                    }
                    if(c->cmsg_level!=SOL_SOCKET || c->cmsg_type!=SCM_RIGHTS ||
                       c->cmsg_len<CMSG_LEN(0)){extra=true;continue;}
                    size_t n=c->cmsg_len-CMSG_LEN(0);
                    if(n!=sizeof(int))extra=true;
                    for(size_t j=0;j+sizeof(int)<=n;j+=sizeof(int)){
                        int v;memcpy(&v,(unsigned char *)CMSG_DATA(c)+j,sizeof(v));
                        if(received<0){received=v;}else{close(v);extra=true;}
                    }
                }
                if(extra || (take!=NULL && (received<0 ||
                   (kind==ZXS_LIFE_FD ? !life_reader(received) :
                    (kind==ZXS_READ_STREAM ? claim_socket_kind(received,false,SOCK_STREAM)!=0 :
                     (kind==ZXS_DATA_FD ? claim_socket(received,false)!=0 : (kind!=ZXS_TREE_FD || zxs_smb_ctl_claim(received)!=0)))))) ||
                   (size_t)got!=len){
                    if(received>=0)close(received);
                    return bad();
                }
            }
        }
        if(got<0 && (errno==EINTR || errno==EAGAIN) && !nowait)continue;
        if(got<0)return -1;
        if(take!=NULL)*take=received;
        return (size_t)got==len?0:bad(); /* no fragment joining, oversize, empty or reflected ack */
    }
}
int zxs_smb_ctl_read(int fd,uint8_t *p,size_t n){return transfer(fd,NULL,p,n,-1,NULL,ZXS_NO_FD,false);}
int zxs_smb_ctl_write(int fd,const uint8_t *p,size_t n){return transfer(fd,p,NULL,n,-1,NULL,ZXS_NO_FD,false);}
/* Return only the write end locally; it stays open through every participating
 * smbd fork. The stdin/stdout of a client and the main SQLite directory are
 * never handed over. Loss of a socket direction is not end of this lease. */
int zxs_smb_ctl_hello(int fd,const uint8_t *bound)
{
    int p[2];if(pipe2(p,O_NONBLOCK|O_CLOEXEC)<0)return -1;
    int rc=transfer(fd,bound,NULL,ZXS_SMB_BIND_LEN,p[0],NULL,ZXS_NO_FD,false),err=errno;close(p[0]);
    if(rc!=0){close(p[1]);errno=err;return -1;}
    return p[1];
}
int zxs_smb_ctl_bound(int fd,uint8_t *bound)
{
    int rd=-1;if(transfer(fd,NULL,bound,ZXS_SMB_BIND_LEN,-1,&rd,ZXS_LIFE_FD,false)!=0)return -1;
    return rd;
}
#include "zxs_smb_workers.inc"
static const uint8_t challenge_tag[16]="ZXS-SAM/CHLNG-1";
/* Only the native new SMB2 GENSEC instance calls this. The shared master
 * control send is a single atomic seqpacket + one endpoint; *no* reply or
 * challenge is ever read from that shared socket by competing children. */
int zxs_smb_try_open(int control,uint64_t net,uint64_t sk,int64_t *started)
{
    int p[2];if(millis(started)<0)return -1;
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)<0)return -1;
    int rc=role_send(control,begin_tag,net,sk,p[1]),err=errno;
    close(p[1]);
    if(rc!=0){close(p[0]);errno=err;return -1;}
    return p[0];
}
static bool within(int64_t start,int64_t limit)
{
    int64_t now;return start>=0 && millis(&now)==0 && now>=start && now-start<limit;
}
int zxs_smb_try_read(int fd,int64_t start,uint8_t out[8])
{
    if(!within(start,10000)){errno=ETIMEDOUT;return -1;}
    uint8_t reply[24],nz=0;
    if(transfer(fd,NULL,reply,sizeof(reply),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(reply,challenge_tag,16)!=0)return bad();
    for(size_t i=16;i<sizeof(reply);i++)nz|=reply[i];
    if(nz==0)return bad();
    memcpy(out,reply+16,8);return 0;
}
int zxs_smb_try_current(int fd,int64_t start)
{
    if(!within(start,30000))return bad();
    return zxs_smb_ctl_live(fd);
}
int zxs_smb_try_issue(int fd,const uint8_t challenge[8])
{
    uint8_t reply[24];memcpy(reply,challenge_tag,16);memcpy(reply+16,challenge,8);
    return transfer(fd,reply,NULL,sizeof(reply),-1,NULL,ZXS_NO_FD,true);
}
int zxs_smb_try_wait(int ctl,int life,const int *attempt,size_t n,uint8_t *events)
{
    if(n>ZXS_SMB_MAX_ATTEMPTS+ZXS_SMB_MAX_SESSIONS)return bad();
    struct pollfd p[2+ZXS_SMB_MAX_ATTEMPTS+ZXS_SMB_MAX_SESSIONS]={0};
    p[0]=(struct pollfd){.fd=ctl,.events=POLLIN|POLLRDHUP};
    p[1]=(struct pollfd){.fd=life,.events=0}; /* ONLY pipe last-write loss */
    for(size_t i=0;i<n;i++)p[i+2]=(struct pollfd){.fd=attempt[i],.events=POLLIN|POLLRDHUP};
    memset(events,0,n+2);
    int r=poll(p,n+2,500);
    if(r<0 && errno==EINTR)return 0;
    if(r<0)return -1;
    for(size_t i=0;i<n+2;i++){
        if(p[i].revents&(POLLNVAL|POLLERR)){
            if(i==1)return bad(); /* an invalid anchor is not normal drain */
            events[i]|=2;
        }
        if(p[i].revents&(POLLHUP|POLLRDHUP))events[i]|=2;
        if(p[i].revents&POLLIN)events[i]|=1;
    }
    return 0;
}
int zxs_smb_ctl_live(int fd)
{
    struct pollfd p={.fd=fd,.events=POLLIN|POLLRDHUP};
    int n;do {n=poll(&p,1,0);}while(n<0 && errno==EINTR);
    /* Called only on the smbd receiving side after its expected reply.
     * The worker sends nothing more here; any data, EOF or error revokes. */
    return n==0?0:bad();
}
int zxs_smb_ctl_drain(int fd){return shutdown(fd,SHUT_WR);}
/* A kernel anonymous pipe has no socket-style half-close. On its read
 * end POLLHUP means the *last* reference to the write end went away. It
 * says nothing about work in another service that never held that end. */
int zxs_smb_ctl_life(int fd,int timeout)
{
    if(timeout!=0 && timeout!=500)return bad();
    struct pollfd p={.fd=fd,.events=0};
    int r=poll(&p,1,timeout);
    if(r<0 && errno==EINTR)return 0;
    if(r<0)return -1;
    if(r==0)return 0;
    if((p.revents&(POLLERR|POLLNVAL))!=0)return bad();
    return (p.revents&POLLHUP)!=0?1:bad();
}

int zxs_smb_bound_fd(int fd)
{
    struct stat st;struct statvfs vs;
    int flags=fcntl(fd,F_GETFL),f=fcntl(fd,F_GETFD);
    if(flags<0 || f<0 || (flags&O_ACCMODE)!=O_RDONLY || (flags&O_PATH)!=0 ||
       fstat(fd,&st)<0 || !S_ISREG(st.st_mode) || st.st_uid!=0 ||
       (st.st_mode&0022)!=0 || st.st_nlink!=1 || st.st_size!=ZXS_SMB_BIND_LEN ||
       fstatvfs(fd,&vs)<0 || (vs.f_flag&ST_RDONLY)==0){return bad();}
    return fcntl(fd,F_SETFD,f|FD_CLOEXEC);
}

/* These two fixed messages carry neither client proof nor signing material.
 * The caller does not supply a pool, user, desired operation or generation. */
static const uint8_t cut_request[16]="ZXS-SAM/GNCUT-1";
static const uint8_t cut_answer[16]="ZXS-SAM/GEN-1";
int zxs_smb_cut_ask(int fd,int64_t started,const uint8_t challenge[8])
{
    uint8_t b[24];
    if(zxs_smb_try_current(fd,started)!=0)return -1;
    memcpy(b,cut_request,16);memcpy(b+16,challenge,8);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_smb_cut_take(int fd,const uint8_t challenge[8])
{
    uint8_t b[24];
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    return memcmp(b,cut_request,16)==0 && memcmp(b+16,challenge,8)==0 ? 0 : bad();
}
int zxs_smb_cut_reply(int fd,const uint8_t challenge[8],const uint8_t id[ZXS_SMB_CUT_BYTES])
{
    uint8_t b[24+ZXS_SMB_CUT_BYTES];
    memcpy(b,cut_answer,16);memcpy(b+16,challenge,8);memcpy(b+24,id,ZXS_SMB_CUT_BYTES);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
static bool cut_id(const uint8_t id[16])
{
    uint8_t n=0;for(size_t i=0;i<16;i++)n|=id[i];return n!=0;
}
int zxs_smb_public_generation(const uint8_t id[ZXS_SMB_CUT_BYTES],struct zxs_smb_generation *out)
{
    static const uint8_t sid_header[]={1,5,0,0,0,0,0,5,21,0,0,0};
    struct zxs_smb_generation tmp={0};
    if(out==NULL || id==NULL ||
       !cut_id(id) || !cut_id(id+16) || !cut_id(id+32) ||
       memcmp(id+56,sid_header,sizeof(sid_header))!=0 || memcmp(id+80,"\xe8\x03\0\0",4)!=0 ||
       id[84]==0 || id[84]>64)return bad();
    uint8_t domain=0;for(size_t i=68;i<80;i++)domain|=id[i];
    if(domain==0)return bad();
    for(size_t i=48;i<56;i++)tmp.generation=(tmp.generation<<8)|id[i];
    if(tmp.generation==0 || tmp.generation>INT64_MAX)return bad();
    for(size_t i=0;i<64;i++){
        uint8_t c=id[85+i];
        if(i>=id[84]){if(c!=0)return bad();continue;}
        if(!((c>='a'&&c<='z')||(c>='0'&&c<='9')||
             (i!=0 && (c=='.'||c=='-'||c=='_'))))return bad();
        tmp.login[i]=(char)c;
    }
    memcpy(tmp.pool,id,16);memcpy(tmp.account,id+16,16);memcpy(tmp.credential_op,id+32,16);
    memcpy(tmp.sid,id+56,28);
    if (!strcmp(tmp.login,"guest") || !strcmp(tmp.login,"anonymous") ||
        !strcmp(tmp.login,"root") || !strcmp(tmp.login,"nobody"))return bad();
    *out=tmp;return 0;
}
int zxs_smb_cut_read(int fd,int64_t started,const uint8_t challenge[8],struct zxs_smb_generation *out)
{
    uint8_t b[24+ZXS_SMB_CUT_BYTES]; struct zxs_smb_generation tmp;
    if(out==NULL || !within(started,30000))return bad();
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(memcmp(b,cut_answer,16)!=0 || memcmp(b+16,challenge,8)!=0 ||
       zxs_smb_public_generation(b+24,&tmp)!=0 || zxs_smb_try_current(fd,started)!=0)return bad();
    *out=tmp;return 0;
}

/* A worker keeps this exact endpoint only in its original live session; no
 * second control listener, client-chosen user, credential, or database path. */
static const uint8_t active_tag[16]="ZXS-SAM/ACTIVE1";
static const uint8_t tree_tag[16]="ZXS-SAM/TREE-1";
static const uint8_t tree_ok[16]="ZXS-SAM/TRROOT-2";
int zxs_smb_active_send(int fd,const uint8_t ch[8])
{
    uint8_t b[24];memcpy(b,active_tag,16);memcpy(b+16,ch,8);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
int zxs_smb_active_take(int fd,const uint8_t ch[8])
{
    uint8_t b[24];
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    return !memcmp(b,active_tag,16)&&!memcmp(b+16,ch,8)?0:bad();
}
int zxs_smb_tree_ask(int session,const uint8_t share[16])
{
    uint8_t b[32];int p[2],rc,e;
    if(share==NULL)return bad();
    if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)<0)return -1;
    memcpy(b,tree_tag,16);memcpy(b+16,share,16);
    rc=transfer(session,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);e=errno;close(p[1]);
    if(rc<0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_smb_tree_take(int session,int worker,uint8_t share[16])
{
    uint8_t b[32];int reply=-1;
    if(transfer(session,NULL,b,sizeof(b),-1,&reply,ZXS_TREE_FD,true)!=0)return -1;
    if(memcmp(b,tree_tag,16)||zxs_smb_attempt_peer(reply,worker)!=0){
        close(reply);return bad();
    }
    memcpy(share,b+16,16);return reply;
}
int zxs_smb_tree_reply(int fd,const uint8_t decision[ZXS_SMB_TREE_CUT_BYTES])
{
    uint8_t b[16+ZXS_SMB_TREE_CUT_BYTES];memcpy(b,tree_ok,16);memcpy(b+16,decision,ZXS_SMB_TREE_CUT_BYTES);
    return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,true);
}
static int zxs_tree_cut_read(int fd,const uint8_t *share,const uint8_t *pool,struct zxs_smb_tree_cut *out)
{
    uint8_t b[16+ZXS_SMB_TREE_CUT_BYTES];struct zxs_smb_tree_cut x={0};
    if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,true)!=0)return -1;
    if(out==NULL || memcmp(b,tree_ok,16)!=0 || !cut_id(b+73) ||
       (share!=NULL && memcmp(b+16,share,16)!=0) ||
       (pool!=NULL && memcmp(b+73,pool,16)!=0))return bad();
    if(share!=NULL && !cut_id(share)){
        const uint8_t zero[57]={0};
        if(memcmp(b+16,zero,sizeof(zero))!=0 || b[89]!=0)return bad();
        memcpy(x.pool,b+73,16);x.root_fd=-1;*out=x;return 0;
    }
    if(!cut_id(b+16)||!cut_id(b+32)||!cut_id(b+48)||(b[72]!=1&&b[72]!=2)||
       b[89]<2||b[89]>=64)return bad();
    for(size_t i=64;i<72;i++)x.revision=(x.revision<<8)|b[i];
    if(x.revision==0 || x.revision>INT64_MAX)return bad();
    memcpy(x.share,b+16,16);memcpy(x.volume,b+32,16);memcpy(x.setting,b+48,16);
    x.mode=b[72];memcpy(x.pool,b+73,16);x.volume_ordinal=b[89];x.root_fd=-1;
    *out=x;return 0;
}
int zxs_smb_tree_read(int fd,const uint8_t share[16],struct zxs_smb_tree_cut *out)
{
    if(share==NULL)return bad();
    return zxs_tree_cut_read(fd,share,NULL,out);
}
int zxs_ns_folder_read(int fd,const uint8_t pool[16],struct zxs_smb_tree_cut *out)
{
    if(pool==NULL||!cut_id(pool))return bad();
    return zxs_tree_cut_read(fd,NULL,pool,out);
}
/* Storage-side only. Never consume another tree request on the session
 * socket. Its readable data is for the master dispatcher, not a close. */
int zxs_smb_root_wait(int tree,int session,int timeout)
{
    if(timeout!=0 && timeout!=500)return bad();
    struct pollfd p[2]={{.fd=tree,.events=POLLIN|POLLRDHUP},{.fd=session,.events=POLLRDHUP}};
    int r;
    do{r=poll(p,2,timeout);}while(r<0 && errno==EINTR && timeout==0);
    if(r<0 && errno==EINTR)return 0;
    if(r<0)return -1;
    if(p[1].revents!=0 || (p[0].revents&~POLLIN)!=0)return -1;
    return p[0].revents==POLLIN?1:0; /* data is consumed only by this tree worker */
}
int zxs_smb_root_release(struct zxs_smb_tree_cut *owned)
{
    if(owned!=NULL && owned->root_fd>=0){
        shutdown(owned->root_fd,SHUT_RDWR);
        close(owned->root_fd);owned->root_fd=-1;
    }
    return 0;
}

#include "zxs_plan_wire.inc"
#include "zxs_directory_wire.inc"
#include "zxs_read_wire.inc"
#include "zxs_work_wire.inc"
