/* SPDX-License-Identifier: Apache-2.0
 * Linux 6.12, Debian amd64. Private, nonsecret process registrations.
 * Generated SCM_PIDFD identifies the actual sender in this launch: its
 * participants may not hold or regain CAP_SYS_ADMIN in the PID namespace.
 * SO_PEERPIDFD alone identifies the creator of a socketpair, not the sender.
 * This code has no account material and accepts no client credential.
 */
#include <linux/capability.h>
#include <signal.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#ifndef SO_PASSPIDFD
#define SO_PASSPIDFD 76
#endif
#ifndef SO_PEERPIDFD
#define SO_PEERPIDFD 77
#endif
#ifndef SCM_PIDFD
#define SCM_PIDFD 4
#endif
#ifndef PID_FS_MAGIC
#define PID_FS_MAGIC 0x50494446
#endif
static const uint8_t dispatch_tag[16]="ZXS-SAM/CHILD-1", begin_tag[16]="ZXS-SAM/BEGIN-2";

int zxs_smb_sender_isolated(void)
{
    struct __user_cap_header_struct h={.version=_LINUX_CAPABILITY_VERSION_3,.pid=0};
    struct __user_cap_data_struct d[2]={{0}};
    const unsigned bit=1U<<(CAP_SYS_ADMIN%32);
    if(prctl(PR_CAPBSET_READ,CAP_SYS_ADMIN,0,0,0)!=0 ||
       syscall(SYS_capget,&h,d)!=0 ||
       ((d[CAP_SYS_ADMIN/32].effective|d[CAP_SYS_ADMIN/32].permitted|
         d[CAP_SYS_ADMIN/32].inheritable)&bit)!=0 ||
       prctl(PR_CAP_AMBIENT,PR_CAP_AMBIENT_IS_SET,CAP_SYS_ADMIN,0,0)!=0){
        errno=EPERM;return -1;
    }
    return 0;
}
int zxs_smb_process_live(int fd)
{
    struct statfs fs;struct pollfd p={.fd=fd,.events=POLLIN};
    int flags=fcntl(fd,F_GETFL);
    int ready;
    if(flags<0 || (flags&O_PATH)!=0 || fstatfs(fd,&fs)!=0 ||
       (unsigned long)fs.f_type!=PID_FS_MAGIC)return bad();
    do{ready=poll(&p,1,0);}while(ready<0 && errno==EINTR);
    if(ready!=0)return bad();
    return 0;
}
int zxs_smb_process_claim(int fd)
{
    int flags=fcntl(fd,F_GETFD);
    if(flags<0 || zxs_smb_process_live(fd)!=0)return -1;
    return fcntl(fd,F_SETFD,flags|FD_CLOEXEC);
}
int zxs_smb_same_process(int a,int b)
{
    struct stat x,y;
    if(zxs_smb_process_live(a)!=0 || zxs_smb_process_live(b)!=0 ||
       fstat(a,&x)!=0 || fstat(b,&y)!=0)return -1;
    return x.st_dev==y.st_dev && x.st_ino!=0 && x.st_ino==y.st_ino;
}
static int same_endpoint_process(int endpoint,int sender,bool root_creator)
{
    int peer=-1;socklen_t n=sizeof(peer);
    if(claim_socket(endpoint,root_creator)!=0 ||
       getsockopt(endpoint,SOL_SOCKET,SO_PEERPIDFD,&peer,&n)!=0)return -1;
    bool good=n==sizeof(peer) && peer>=0 && zxs_smb_same_process(peer,sender)==1;
    if(peer>=0)close(peer);
    return good?0:bad();
}
int zxs_smb_attempt_peer(int endpoint,int sender){return same_endpoint_process(endpoint,sender,true);}
int zxs_smb_origin_enable(int fd)
{
    int one=1;
    if(!opt(fd,SO_PASSCRED,0) || setsockopt(fd,SOL_SOCKET,SO_PASSPIDFD,&one,sizeof(one))!=0 ||
       !opt(fd,SO_PASSPIDFD,1))return bad();
    return 0;
}
static int role_send(int fd,const uint8_t tag[16],uint64_t net,uint64_t sk,int right)
{
    uint8_t buf[32];uint64_t v[2]={net,sk};
    if(net==0 || sk==0 || right<0)return bad();
    memcpy(buf,tag,16);
    for(size_t i=0;i<2;++i)for(size_t j=0;j<8;++j)buf[16+8*i+j]=(uint8_t)(v[i]>>(56-8*j));
    /* Explicitly supply only our own real credentials. The kernel pins
     * this sender even if our packet beats storage's post-READY setsockopt.
     * The receiver enables PASSPIDFD only after its last send; enabling it
     * before sending READY would autobind an otherwise unnamed socket. */
    struct iovec io={.iov_base=buf,.iov_len=sizeof(buf)};
    union{struct cmsghdr align;unsigned char bytes[CMSG_SPACE(sizeof(int))+CMSG_SPACE(sizeof(struct ucred))];} u={0};
    struct msghdr m={.msg_iov=&io,.msg_iovlen=1,.msg_control=u.bytes,.msg_controllen=sizeof(u.bytes)};
    struct cmsghdr *c=CMSG_FIRSTHDR(&m);c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_RIGHTS;
    c->cmsg_len=CMSG_LEN(sizeof(int));memcpy(CMSG_DATA(c),&right,sizeof(right));
    c=CMSG_NXTHDR(&m,c);if(c==NULL)return bad();
    struct ucred who={.pid=getpid(),.uid=getuid(),.gid=getgid()};
    c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_CREDENTIALS;c->cmsg_len=CMSG_LEN(sizeof(who));
    memcpy(CMSG_DATA(c),&who,sizeof(who));
    ssize_t n=sendmsg(fd,&m,MSG_NOSIGNAL|MSG_DONTWAIT);
    return n==(ssize_t)sizeof(buf)?0:(n<0?-1:bad());
}
/* Called by the one real parent immediately after fork, in that same tevent
 * callback, before its SIGCHLD event can reap or another accept can fork.
 * waitid on the pidfd verifies that this still-live child belongs to us.
 * No polling/waiting for storage or for the child in the accept loop. */
int zxs_smb_dispatch(int ctl,pid_t child,uint64_t net,uint64_t sk)
{
    struct sigaction sa;siginfo_t info={0};
    if(child<=0 || sigaction(SIGCHLD,NULL,&sa)!=0 || sa.sa_handler==SIG_IGN ||
       (sa.sa_flags&SA_NOCLDWAIT)!=0 || zxs_smb_sender_isolated()!=0)return bad();
    int pidfd=(int)syscall(SYS_pidfd_open,child,0);
    if(pidfd<0)return -1;
    int r=-1;
    if(waitid(P_PIDFD,(id_t)pidfd,&info,WEXITED|WNOHANG|WNOWAIT)==0 &&
       info.si_pid==0 && zxs_smb_process_live(pidfd)==0){
        r=role_send(ctl,dispatch_tag,net,sk,pidfd);
    }else{errno=ECHILD;}
    int e=errno;close(pidfd);errno=e;return r;
}
/* One intact tag + original TCP tuple, one SCM_RIGHTS, one kernel-generated
 * SCM_PIDFD. Nothing received is adopted after extra/truncated control data.
 * A local fd shortage for a recognized frame drops only its own request. */
int zxs_smb_origin_recv(int fd,uint64_t tcp[2],int *from,int *right)
{
    uint8_t buf[32];struct iovec io={.iov_base=buf,.iov_len=sizeof(buf)};
    union{struct cmsghdr align;unsigned char bytes[CMSG_SPACE(4*sizeof(int))+CMSG_SPACE(sizeof(int))];} u={0};
    struct msghdr msg={.msg_iov=&io,.msg_iovlen=1,.msg_control=u.bytes,.msg_controllen=sizeof(u.bytes)};
    int sender=-1,object=-1;
    ssize_t n=recvmsg(fd,&msg,MSG_TRUNC|MSG_DONTWAIT|MSG_CMSG_CLOEXEC);
    if(n<0)return -1;
    bool extra=(msg.msg_flags&(MSG_CTRUNC|MSG_TRUNC))!=0;
    for(struct cmsghdr *c=CMSG_FIRSTHDR(&msg);c!=NULL;c=CMSG_NXTHDR(&msg,c)){
        bool source=c->cmsg_level==SOL_SOCKET && c->cmsg_type==SCM_PIDFD;
        bool rights=c->cmsg_level==SOL_SOCKET && c->cmsg_type==SCM_RIGHTS;
        if((!source && !rights)||c->cmsg_len<CMSG_LEN(0)){extra=true;continue;}
        size_t bytes=c->cmsg_len-CMSG_LEN(0);if(bytes!=sizeof(int))extra=true;
        int *slot=source?&sender:&object;
        for(size_t i=0;i+sizeof(int)<=bytes;i+=sizeof(int)){
            int v;memcpy(&v,(uint8_t *)CMSG_DATA(c)+i,sizeof(v));
            if(v<0){extra=true;}else if(*slot>=0){close(v);extra=true;}else{*slot=v;}
        }
    }
    int kind=0;
    if(n==(ssize_t)sizeof(buf) && !(msg.msg_flags&MSG_TRUNC)){
        if(memcmp(buf,dispatch_tag,16)==0)kind=1;
        if(memcmp(buf,begin_tag,16)==0)kind=2;
    }
    if(kind!=0 && !extra && object>=0 && sender>=0 && zxs_smb_process_live(sender)==0){
        for(size_t i=0;i<2;++i){tcp[i]=0;for(size_t j=0;j<8;++j)tcp[i]=(tcp[i]<<8)|buf[16+8*i+j];}
        if(tcp[0]!=0 && tcp[1]!=0){
            *from=sender;*right=object;return kind;
        }
    }
    if(sender>=0)close(sender);
    if(object>=0)close(object);
    if(kind!=0){errno=EBADMSG;return -2;}
    return bad();
}
