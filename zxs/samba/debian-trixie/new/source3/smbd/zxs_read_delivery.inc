/* SPDX-License-Identifier: GPL-3.0-or-later
 * One read on the real native open. Only the final confirmation authorizes
 * releasing the private buffer to Samba's normal signing/network path. */
struct zxs_read_delivery {
    struct tevent_context *ev;
    struct tevent_req *packet;
    struct timeval deadline;
    size_t size;
    int fd;
    bool got_header;
    uint8_t *bytes;
};
static int zxs_read_end(struct zxs_read_delivery *d)
{
    TALLOC_FREE(d->packet);
    if(d->fd>=0){close(d->fd);d->fd=-1;}
    return 0;
}
static void zxs_packet_done(struct tevent_req *sub)
{
    struct tevent_req *req=tevent_req_callback_data(sub,struct tevent_req);
    struct zxs_read_delivery *d=tevent_req_data(req,struct zxs_read_delivery);
    uint8_t *buf=NULL;int error=0;
    ssize_t n=read_packet_recv(sub,d,&buf,&error);
    d->packet=NULL;TALLOC_FREE(sub);
    if(n<0)goto fail;
    if(!d->got_header){
        if(n!=16 || memcmp(buf,ZXS_READ_HEADER,16)!=0)goto fail;
        TALLOC_FREE(buf);d->got_header=true;
        d->packet=read_packet_send(d,d->ev,d->fd,d->size+16,NULL,NULL);
        if(d->packet==NULL)goto fail;
        if(!tevent_req_set_endtime(d->packet,d->ev,d->deadline))goto fail;
        tevent_req_set_callback(d->packet,zxs_packet_done,req);return;
    }
    if((size_t)n!=d->size+16 || memcmp(buf+d->size,ZXS_READ_COMMIT,16)!=0)goto fail;
    d->bytes=buf;zxs_read_end(d);tevent_req_done(req);return;
fail:
    zxs_read_end(d);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);
}
static struct tevent_req *zxs_delivery_send(TALLOC_CTX *ctx,struct tevent_context *ev,
        int member,uint64_t offset,uint32_t count,size_t length)
{
    struct zxs_read_delivery *d;struct tevent_req *req=tevent_req_create(ctx,&d,struct zxs_read_delivery);
    if(req==NULL)return NULL;
    d->fd=-1;d->ev=ev;d->size=length;d->deadline=timeval_current_ofs(15,0);
    talloc_set_destructor(d,zxs_read_end);
    if(length>count || count>ZXS_READ_LIMIT || (d->fd=zxs_read_query(member,offset,count))<0)goto fail;
    d->packet=read_packet_send(d,ev,d->fd,16,NULL,NULL);
    if(d->packet==NULL || !tevent_req_set_endtime(d->packet,ev,d->deadline))goto fail;
    tevent_req_set_callback(d->packet,zxs_packet_done,req);return req;
fail:
    zxs_read_end(d);tevent_req_nterror(req,NT_STATUS_ACCESS_DENIED);return tevent_req_post(req,ev);
}
static NTSTATUS zxs_delivery_recv(struct tevent_req *req,TALLOC_CTX *ctx,uint8_t **bytes,size_t *size)
{
    struct zxs_read_delivery *d=tevent_req_data(req,struct zxs_read_delivery);
    NTSTATUS status=NT_STATUS_OK;
    if (!tevent_req_is_nterror(req,&status)) {
        *size=d->size;*bytes=talloc_move(ctx,&d->bytes);
    }
    tevent_req_received(req);
    return status;
}
