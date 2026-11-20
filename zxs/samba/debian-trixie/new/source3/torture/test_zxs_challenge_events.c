/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source qualification only. Use the same tevent primitive and receipt as
 * the dedicated SESSION_SETUP wrapper. No daemon, authentication, credentials
 * or external endpoint; two independent local pairs simulate storage delay. */
#include "includes.h"
#include "../lib/async_req/async_sock.h"
#include "param/zxs_smb_control.h"
#include <sys/socket.h>
#include <time.h>

struct observed { int fd; int64_t start; uint8_t want; bool done,ok; };
static void notified(struct tevent_req *req)
{
	struct observed *o=tevent_req_callback_data_void(req);
	uint8_t got[8];
	int error=0;
	o->done=true;
	o->ok=wait_for_read_recv(req,&error);
	TALLOC_FREE(req);
	if (!o->ok || zxs_smb_try_read(o->fd,o->start,got)!=0 ||
	    zxs_smb_try_current(o->fd,o->start)!=0) { o->ok=false;return; }
	for (size_t i=0;i<sizeof(got);i++) {
		if (got[i]!=o->want) { o->ok=false;return; }
	}
}
static bool watch(struct tevent_context *ev,struct observed *o)
{
	struct tevent_req *r=wait_for_read_send(ev,ev,o->fd,false);
	if (!r || !tevent_req_set_endtime(r,ev,tevent_timeval_current_ofs(2,0))) { return false; }
	tevent_req_set_callback(r,notified,o);return true;
}
/* This part covers the two *reply* sockets, not native login. It uses
 * public dummy numbers only and does not require a database or peer UID. */
static bool separate_tree_replies(void)
{
	int a[2],b[2];uint8_t wanted[16]={1},other[16]={2},cut[ZXS_SMB_TREE_CUT_BYTES]={0};
	struct zxs_smb_tree_cut decision={0};
	if (socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,a)!=0 ||
	    socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,b)!=0) { return false; }
	memcpy(cut,wanted,16);cut[16]=3;cut[32]=4;cut[55]=9;cut[56]=1;
	bool ok=zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(b[0],wanted,&decision)!=0 &&
		zxs_smb_tree_read(a[0],wanted,&decision)==0 && decision.volume[0]==3 &&
		decision.setting[0]==4 && decision.revision==9 && decision.mode==1;
	ok=ok && zxs_smb_tree_reply(b[1],cut)==0 && zxs_smb_tree_read(b[0],other,&decision)!=0;
	cut[56]=0;ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(a[0],wanted,&decision)!=0;
	close(a[0]);close(a[1]);close(b[0]);close(b[1]);return ok;
}
int main(void)
{
	struct tevent_context *ev=tevent_context_init(NULL);
	int a[2],b[2];struct timespec now;int64_t start;
	uint8_t x[8],y[8];
	if (!ev || !separate_tree_replies() || socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,a)!=0 ||
	    socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,b)!=0 ||
	    clock_gettime(CLOCK_MONOTONIC,&now)!=0) {return 1;}
	start=(int64_t)now.tv_sec*1000+now.tv_nsec/1000000;
	struct observed older={.fd=a[0],.start=start,.want=1};
	struct observed other={.fd=b[0],.start=start,.want=2};
	memset(x,1,sizeof(x));memset(y,2,sizeof(y));
	if (!watch(ev,&older) || !watch(ev,&other) || zxs_smb_try_issue(b[1],y)!=0) {return 1;}
	while (!other.done) { if (tevent_loop_once(ev)!=0) {return 1;} }
	if (!other.ok || older.done || zxs_smb_try_issue(a[1],x)!=0) {return 1;}
	while (!older.done) { if (tevent_loop_once(ev)!=0) {return 1;} }
	/* Its storage endpoint is still open; local expiry, not peer HUP,
	 * must stop reuse even if the worker itself has not been scheduled. */
	if (!older.ok || zxs_smb_ctl_live(a[0])!=0 ||
	    zxs_smb_try_current(a[0],start-30000)==0) {return 1;}
	/* The last public generation cut uses only this original endpoint.
	 * No real credential is computed and none of these values logs in. */
	uint8_t id[ZXS_SMB_CUT_BYTES]={0};
	struct zxs_smb_generation seen={0};
	id[0]=1;id[16]=2;id[32]=3;id[55]=1;
	id[56]=1;id[57]=5;id[63]=5;id[64]=21;id[68]=1;id[80]=0xe8;id[81]=3;
	id[84]=4;memcpy(id+85,"user",4);
	if (zxs_smb_cut_ask(b[0],start,y)!=0 || zxs_smb_cut_take(b[1],y)!=0 ||
	    zxs_smb_cut_reply(b[1],y,id)!=0 || zxs_smb_cut_read(b[0],start,y,&seen)!=0 ||
	    seen.pool[0]!=1 || seen.account[0]!=2 || seen.credential_op[0]!=3 ||
	    seen.generation!=1 || strcmp(seen.login,"user")!=0) {return 1;}
	id[148]=1; /* noncanonical trailing byte */
	if (zxs_smb_cut_reply(a[1],x,id)!=0 || zxs_smb_cut_read(a[0],start,x,&seen)==0 ||
	    seen.account[0]!=2) {return 1;}
	id[148]=0;
	if (zxs_smb_cut_reply(a[1],x,id)!=0 || zxs_smb_cut_read(a[0],start,y,&seen)==0) {return 1;}
	TALLOC_FREE(ev);close(a[0]);close(a[1]);close(b[0]);close(b[1]);return 0;
}
