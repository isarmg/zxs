/* SPDX-License-Identifier: Apache-2.0
 * One admitted FILE FSP's private command edge. One endpoint and bounded
 * header; bytes/EOF and response move only on its private stream.
 */
static const uint8_t work_query_tag[16]="ZXS-FILE/WORK-1";
static const uint8_t work_done_tag[16]="ZXS-FILE/DONE-1";

static bool work_valid(uint8_t command,uint64_t offset,uint32_t count,uint8_t settle)
{
    if (offset>INT64_MAX || count>ZXS_READ_LIMIT || offset>(uint64_t)INT64_MAX-count ||
        settle>1) return false;
    switch(command){
    case ZXS_WORK_WRITE: return true;
    case ZXS_WORK_EOF: return count==0 && settle==0;
    case ZXS_WORK_FLUSH: return offset==0 && count==0 && settle==0;
    default: return false;
    }
}

int zxs_work_query(int member,uint8_t command,uint64_t offset,uint32_t count,uint8_t settle)
{
    uint8_t b[32]={0};int p[2],rc,e;
    if(!work_valid(command,offset,count,settle))return bad();
    memcpy(b,work_query_tag,16);
    for(size_t i=0;i<8;i++)b[16+i]=(uint8_t)(offset>>(56-8*i));
    for(size_t i=0;i<4;i++)b[24+i]=(uint8_t)(count>>(24-8*i));
    b[28]=command;b[29]=settle;
    if(socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,p)!=0)return -1;
    rc=transfer(member,b,NULL,sizeof(b),p[1],NULL,ZXS_NO_FD,true);e=errno;close(p[1]);
    if(rc!=0){close(p[0]);errno=e;return -1;}return p[0];
}
int zxs_work_take(int member,int worker,uint8_t *command,uint64_t *offset,uint32_t *count,uint8_t *settle)
{
    uint8_t b[32];int answer=file_stream_take(member,worker,b);
    if(answer<0)return -1;
    uint64_t off=zxs_member_u64(b+16);uint32_t n=file_count(b);
    if(memcmp(b,read_query_tag,16)==0 && memcmp(b+28,"\0\0\0\0",4)==0 &&
       n<=ZXS_READ_LIMIT && off<=(uint64_t)INT64_MAX-n){
        *command=ZXS_WORK_READ;*offset=off;*count=n;*settle=0;return answer;
    }
    if(memcmp(b,work_query_tag,16)!=0||b[30]!=0||b[31]!=0||!work_valid(b[28],off,n,b[29])){
        close(answer);return bad();
    }
    *command=b[28];*offset=off;*count=n;*settle=b[29];return answer;
}

/* Writer worker only. Client half-close is the exact end of one request;
 * it is expected and must not be confused with loss of the reply peer. */
int zxs_work_answer_live(int fd)
{
    struct pollfd p={.fd=fd,.events=0};int n;
    do{n=poll(&p,1,0);}while(n<0&&errno==EINTR);
    return n==0?0:bad();
}
int zxs_work_input(int fd,uint8_t *out,size_t n)
{
    int64_t start;if(n>ZXS_READ_LIMIT||(!out&&n)||millis(&start)<0)return bad();
    for(;;){
        uint8_t extra;
        struct pollfd p={.fd=fd,.events=POLLIN};int64_t now;
        if(millis(&now)<0||now>=start+10000){errno=ETIMEDOUT;return -1;}
        int rc=poll(&p,1,(int)(start+10000-now));
        if(rc<0&&errno==EINTR)continue;
        if(rc<=0||(p.revents&~POLLIN)!=0)return bad();
        ssize_t got=recv(fd,n?out:&extra,n?n:1,MSG_DONTWAIT);
        if(got<0&&(errno==EINTR||errno==EAGAIN))continue;
        if(!n)return got==0?zxs_work_answer_live(fd):bad();
        if(got<=0)return bad();
        out+=(size_t)got;n-=(size_t)got;
    }
}
int zxs_work_reply(int fd,uint8_t status,uint64_t eof)
{
    uint8_t b[ZXS_WORK_REPLY]={0};
    if(status>ZXS_WORK_MALFORMED||eof>INT64_MAX||(status!=0&&eof!=0)||zxs_work_answer_live(fd)!=0)return bad();
    memcpy(b,work_done_tag,16);
    for(size_t i=0;i<8;i++)b[16+i]=(uint8_t)(eof>>(56-8*i));
    b[24]=status;
    ssize_t n=send(fd,b,sizeof(b),MSG_DONTWAIT|MSG_NOSIGNAL);
    return n==(ssize_t)sizeof(b)?0:bad();
}
int zxs_work_result(const uint8_t b[ZXS_WORK_REPLY],uint8_t *status,uint64_t *eof)
{
    uint64_t n=zxs_member_u64(b+16);uint8_t op=b[24];
    if(memcmp(b,work_done_tag,16)!=0||memcmp(b+25,"\0\0\0\0\0\0\0",7)!=0||
       op>ZXS_WORK_MALFORMED||n>INT64_MAX||(op!=0&&n!=0))return bad();
    *eof=n;*status=op;return 0;
}
