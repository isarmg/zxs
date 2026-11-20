/* SPDX-License-Identifier: Apache-2.0
 * Original private FSP endpoint. Only a newly-issued, unnamed socket from
 * that same accepted kernel process is a reply destination. */
static const uint8_t read_query_tag[16]="ZXS-FILE/READ-1";
static const uint8_t read_data_tag[16]=ZXS_READ_HEADER;
static const uint8_t read_commit_tag[16]=ZXS_READ_COMMIT;

int zxs_member_held(int fd)
{
    struct pollfd p={.fd=fd,.events=POLLRDHUP};int r;
    do{r=poll(&p,1,0);}while(r<0&&errno==EINTR);
    return r==0?0:bad(); /* Other legitimate queued READs are not EOF. */
}
int zxs_tree_events(int tree,int session,const int *members,size_t n,uint8_t *events)
{
    struct pollfd p[18]={{.fd=tree,.events=POLLIN|POLLRDHUP},{.fd=session,.events=POLLRDHUP}};
    if(n>16)return bad();
    for(size_t i=0;i<n;i++)p[2+i]=(struct pollfd){.fd=members[i],.events=POLLIN|POLLRDHUP};
    memset(events,0,17);
    int r=poll(p,n+2,500);if(r<0&&errno==EINTR)return 0;
    if(r<0||p[1].revents||(p[0].revents&~POLLIN))return -1;
    events[0]=(p[0].revents&POLLIN)?1:0;
    for(size_t i=0;i<n;i++){
        short e=p[2+i].revents;events[i+1]=(e&~POLLIN)?2:((e&POLLIN)?1:0);
    }
    return 0;
}
int zxs_read_query(int member,uint64_t offset,uint32_t count)
{
    uint8_t b[32]={0};int p[2],rc,e;
    if(count>ZXS_READ_LIMIT||offset>(uint64_t)INT64_MAX-count)return bad();
    memcpy(b,read_query_tag,16);
    for(size_t i=0;i<8;i++)b[16+i]=(uint8_t)(offset>>(56-8*i));
    for(size_t i=0;i<4;i++)b[24+i]=(uint8_t)(count>>(24-8*i));
    if(socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=transfer(member,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);e=errno;close(p[1]);
    if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
static int file_stream_take(int member,int worker,uint8_t b[32])
{
    int answer=-1,peer=-1;socklen_t n=sizeof(peer);
    if(transfer(member,NULL,b,32,-1,&answer,ZXS_READ_STREAM,true)!=0)return -1;
    bool good=getsockopt(answer,SOL_SOCKET,SO_PEERPIDFD,&peer,&n)==0 && n==sizeof(peer) &&
        peer>=0 && zxs_smb_same_process(peer,worker)==1;
    if(peer>=0)close(peer);
    if(!good){close(answer);return bad();}return answer;
}
static uint32_t file_count(const uint8_t b[32])
{
    uint32_t len=0;for(size_t i=0;i<4;i++)len=(len<<8)|b[24+i];return len;
}
int zxs_read_take(int member,int worker,uint64_t *offset,uint32_t *count)
{
    uint8_t b[32];int answer=file_stream_take(member,worker,b);
    if(answer<0)return -1;
    uint64_t off=zxs_member_u64(b+16);uint32_t len=file_count(b);
    if(memcmp(b,read_query_tag,16)!=0||memcmp(b+28,"\0\0\0\0",4)!=0||
       len>ZXS_READ_LIMIT||off>(uint64_t)INT64_MAX-len){close(answer);return bad();}
    *offset=off;*count=len;return answer;
}
/* Only on the dedicated tree's storage thread, never inside a SQL transaction
 * or the network event loop. A stalled or canceled receiver gets no success
 * marker; other trees and the supervisor do not wait on this transfer. */
static int read_stream_write(int fd,const uint8_t *p,size_t n,int64_t deadline)
{
    while(n!=0){
        struct pollfd w={.fd=fd,.events=POLLOUT|POLLRDHUP};int64_t now;
        if(millis(&now)<0 || now>=deadline){errno=ETIMEDOUT;return -1;}
        int rc=poll(&w,1,(int)(deadline-now));if(rc<0&&errno==EINTR)continue;
        if(rc<=0 || w.revents!=POLLOUT)return bad();
        ssize_t wrote=send(fd,p,n,MSG_NOSIGNAL|MSG_DONTWAIT);
        if(wrote<0&&(errno==EINTR||errno==EAGAIN))continue;
        if(wrote<=0)return -1;
        p+=(size_t)wrote;n-=(size_t)wrote;
    }
    return 0;
}
int zxs_read_fill(int fd,const uint8_t *bytes,size_t n)
{
    int64_t start;if(n>ZXS_READ_LIMIT||(!bytes&&n))return bad();
    if(millis(&start)<0||read_stream_write(fd,read_data_tag,16,start+10000)!=0)return -1;
    if(read_stream_write(fd,bytes,n,start+10000)!=0)return -1;
    /* Reserve an opportunity for the small trailer before the subsequent
     * grant cut. There is no second writer. Never wait after that cut. */
    for(;;){
        struct pollfd p={.fd=fd,.events=POLLOUT|POLLRDHUP};int64_t now;
        if(millis(&now)<0 || now>=start+10000){errno=ETIMEDOUT;return -1;}
        int rc=poll(&p,1,(int)(start+10000-now));
        if(rc<0 && errno==EINTR)continue;
        return rc==1 && p.revents==POLLOUT?0:bad();
    }
}
int zxs_read_accept(int fd)
{
    /* A short or failed send cannot validate a reply: the native receiver
     * requires all sixteen marker bytes after exactly its expected span. */
    ssize_t n=send(fd,read_commit_tag,sizeof(read_commit_tag),MSG_DONTWAIT|MSG_NOSIGNAL);
    return n==(ssize_t)sizeof(read_commit_tag)?0:-1;
}
