/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source qualification only. Use the same tevent primitive and receipt as
 * the dedicated SESSION_SETUP wrapper. No daemon, authentication, credentials
 * or external endpoint; two independent local pairs simulate storage delay. */
#include "includes.h"
#include "../lib/async_req/async_sock.h"
#include "param/zxs_smb_control.h"
#include <sys/socket.h>
#include <time.h>
#include <sys/syscall.h>

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
	memcpy(cut,wanted,16);cut[16]=3;cut[32]=4;cut[55]=9;cut[56]=1;cut[57]=5;cut[73]=2;
	bool ok=zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(b[0],wanted,&decision)!=0 &&
		zxs_smb_tree_read(a[0],wanted,&decision)==0 && decision.volume[0]==3 &&
		decision.setting[0]==4 && decision.revision==9 && decision.mode==1 &&
		decision.pool[0]==5 && decision.volume_ordinal==2 && decision.root_fd==-1;
	ok=ok && zxs_smb_tree_reply(b[1],cut)==0 && zxs_smb_tree_read(b[0],other,&decision)!=0;
	cut[73]=1;ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(a[0],wanted,&decision)!=0;
	cut[73]=2;cut[56]=0;ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(a[0],wanted,&decision)!=0;
	/* A retained folder is not an ordinary share grant and has no
	 * writable choice, even carrying an exact old deletion ID. */
	memset(cut,0,sizeof(cut));cut[16]=3;cut[32]=7;cut[55]=1;cut[56]=1;cut[57]=5;cut[73]=1;
	ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_ns_folder_read(a[0],cut+57,&decision)==0 &&
		decision.volume_ordinal==1 && decision.setting[0]==7 && decision.mode==1;
	ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_smb_tree_read(a[0],wanted,&decision)!=0;
	cut[56]=2;ok=ok && zxs_smb_tree_reply(a[1],cut)==0 && zxs_ns_folder_read(a[0],cut+57,&decision)!=0;
	close(a[0]);close(a[1]);close(b[0]);close(b[1]);return ok;
}
/* No login or live server here: these are the actual private page transport
 * functions on one kernel pair. Keep two overlapping replies distinct. */
static bool directory_pairs(void)
{
	int p[2],proc=-1,a=-1,b=-1,reply=-1;
	uint8_t after[ZXS_DIR_NAME+1]={0},limit=42,rows[ZXS_DIR_PACKET-16]={0},got[ZXS_DIR_PACKET-16]={0};
	bool ok=false;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return false;
	proc=(int)syscall(SYS_pidfd_open,getpid(),0);if(proc<0)goto done;
	a=zxs_dir_query(p[0],1,(const uint8_t *)"alpha",5);
	b=zxs_dir_query(p[0],0,NULL,0);
	if(a<0||b<0||zxs_dir_query(p[0],17,NULL,0)>=0||zxs_dir_query(p[0],0,(const uint8_t *)"a",1)>=0)goto done;
	reply=zxs_dir_take(p[1],proc,&limit,after);
	if(reply<0||limit!=1||after[0]!=5||memcmp(after+1,"alpha",5)!=0)goto done;
	rows[0]=1;rows[1]=1;rows[2]='z';rows[1+1+ZXS_DIR_NAME+6]=1; /* file 256 */
	if(zxs_dir_reply(reply,rows)!=0||zxs_dir_result(a,1,got)!=0||memcmp(rows,got,sizeof(got))!=0)goto done;
	close(reply);reply=zxs_dir_take(p[1],proc,&limit,after);
	if(reply<0||limit!=0||after[0]!=0)goto done;
	/* Cannot inject an unauthorized entry into a zero-row authority cut. */
	if(zxs_dir_reply(reply,rows)!=0||zxs_dir_result(b,0,got)>=0)goto done;
	/* The top tree admits its single logical R (ordinal 1 -> number 2),
	 * not S (number 1) or the same reserved number from an ordinary lane. */
	rows[1+1+ZXS_DIR_NAME+6]=0;rows[1+1+ZXS_DIR_NAME+7]=2;
	if(zxs_ns_dir_reply(reply,rows)!=0||zxs_ns_dir_result(b,1,got)!=0)goto done;
	if(zxs_dir_reply(reply,rows)!=0||zxs_dir_result(b,1,got)>=0)goto done;
	rows[1+1+ZXS_DIR_NAME+7]=1;
	if(zxs_ns_dir_reply(reply,rows)!=0||zxs_ns_dir_result(b,1,got)>=0)goto done;
	memset(rows,0,sizeof(rows));rows[0]=1;rows[1]=32;
	memcpy(rows+2,"1234567890abcdef0000000000000001",32);memset(rows+1+1+ZXS_DIR_NAME,0xff,8);
	if(zxs_r_round_rows(reply,rows)!=0||zxs_r_round_result(b,1,got)!=0)goto done;
	if(zxs_dir_reply(reply,rows)!=0||zxs_dir_result(b,1,got)>=0)goto done;
	rows[2]='A';if(zxs_r_round_rows(reply,rows)!=0||zxs_r_round_result(b,1,got)>=0)goto done;
	ok=true;
done:
	if(reply>=0)close(reply);if(a>=0)close(a);if(b>=0)close(b);if(proc>=0)close(proc);
	close(p[0]);close(p[1]);return ok;
}
/* Same-tree request with two independent endpoints, no OS FILE fd. */
static bool held_member_pair(void)
{
	struct zxs_smb_tree_cut root={0};struct zxs_member_info out={0};
	uint8_t raw[ZXS_MEMBER_BYTES]={0},name[ZXS_DIR_NAME+1]={0},op=0;int tree[2],proc=-1,fd=-1,peer=-1;bool ok=false;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,tree)!=0)return false;
	proc=(int)syscall(SYS_pidfd_open,getpid(),0);if(proc<0)goto end;
	fd=zxs_member_query(tree[0],(const uint8_t *)"x",1);if(fd<0)goto end;
	peer=zxs_dir_take(tree[1],proc,&op,name);
	if(peer<0 || op!=ZXS_MEMBER_OPEN || name[0]!=1 || name[1]!='x')goto end;
	root.pool[0]=raw[0]=42;root.volume[0]=raw[16]=9;raw[32]=5;raw[48]=raw[64]=128;
	raw[79]=1;raw[87]=7;
	if(zxs_member_reply(peer,raw)!=0 || zxs_member_result(fd,&root,&out)!=0 ||
	   out.revision!=1 || out.size!=7 || out.number!=(UINT64_C(1)<<63) || out.entry[0]!=5)goto end;
	raw[79]=2;if(zxs_member_reply(peer,raw)!=0 || zxs_member_result(fd,&root,&out)!=0 || out.revision!=2)goto end;
	raw[72]=0x80;if(zxs_member_reply(peer,raw)!=0 || zxs_member_result(fd,&root,&out)==0)goto end;
	raw[72]=0;raw[16]++;if(zxs_member_reply(peer,raw)!=0 || zxs_member_result(fd,&root,&out)==0)goto end;
	raw[16]--;raw[79]=0;if(zxs_member_reply(peer,raw)!=0 || zxs_member_result(fd,&root,&out)==0)goto end;
	if(zxs_smb_ctl_live(peer)!=0)goto end;
	close(fd);fd=-1;if(zxs_smb_ctl_live(peer)==0)goto end;
	close(peer);peer=-1;
	fd=zxs_member_write_query(tree[0],(const uint8_t *)"x",1);if(fd<0)goto end;
	peer=zxs_dir_take(tree[1],proc,&op,name);
	if(peer<0 || op!=ZXS_MEMBER_WRITE_OPEN || name[0]!=1 || name[1]!='x')goto end;
	close(peer);peer=-1;close(fd);fd=-1;
	const uint8_t retained[]="1234567890abcdef0000000000000001\\Report.txt";
	if(zxs_r_member_query(tree[0],(const uint8_t *)"only-a-name",11,true)>=0)goto end;
	fd=zxs_r_member_query(tree[0],retained,sizeof(retained)-1,true);if(fd<0)goto end;
	peer=zxs_dir_take(tree[1],proc,&op,name);
	ok=peer>=0 && op==ZXS_RETAINED_SHARE_DELETE && name[0]==sizeof(retained)-1 &&
		memcmp(name+1,retained,sizeof(retained)-1)==0;
end:
	if(peer>=0)close(peer);if(fd>=0)close(fd);if(proc>=0)close(proc);close(tree[0]);close(tree[1]);return ok;
}
/* Exact work request and incoming byte extent on a real private stream.
 * This is not a network SMB operation or a generated work SAVE. */
static bool staged_writer_pair(void)
{
	int member[2],native=-1,answer=-1,proc=-1;bool ok=false;
	uint8_t request=0,settled=0,body[4]={0},reply[ZXS_WORK_REPLY],status;
	uint32_t count=0;uint64_t off=0,eof=0;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,member)!=0)return false;
	proc=(int)syscall(SYS_pidfd_open,getpid(),0);if(proc<0)goto done;
	if(zxs_work_query(member[0],ZXS_WORK_FLUSH,0,1,0)>=0 ||
	   zxs_work_query(member[0],ZXS_WORK_EOF,UINT64_MAX,0,0)>=0 ||
	   zxs_work_query(member[0],ZXS_WORK_WRITE,INT64_MAX,1,0)>=0)goto done;
	native=zxs_work_query(member[0],ZXS_WORK_WRITE,4096,4,1);if(native<0)goto done;
	answer=zxs_work_take(member[1],proc,&request,&off,&count,&settled);
	if(answer<0||request!=ZXS_WORK_WRITE||off!=4096||count!=4||settled!=1)goto done;
	if(send(native,"test",4,MSG_NOSIGNAL)!=4 || shutdown(native,SHUT_WR)!=0)goto done;
	if(zxs_work_input(answer,body,4)!=0 || memcmp(body,"test",4)!=0)goto done;
	if(zxs_work_answer_live(answer)!=0 || zxs_work_reply(answer,0,4100)!=0)goto done;
	if(recv(native,reply,sizeof(reply),MSG_WAITALL)!=(ssize_t)sizeof(reply)||
	   zxs_work_result(reply,&status,&eof)!=0||status!=0||eof!=4100)goto done;
	reply[31]=1;if(zxs_work_result(reply,&status,&eof)==0)goto done;
	close(native);native=-1;close(answer);answer=-1;

	native=zxs_work_query(member[0],ZXS_WORK_FLUSH,0,0,0);if(native<0)goto done;
	answer=zxs_work_take(member[1],proc,&request,&off,&count,&settled);
	if(answer<0||request!=ZXS_WORK_FLUSH||off!=0||count!=0||settled!=0)goto done;
	if(send(native,"!",1,MSG_NOSIGNAL)!=1 || shutdown(native,SHUT_WR)!=0 || zxs_work_input(answer,NULL,0)==0)goto done;
	close(native);native=-1;
	if(zxs_work_answer_live(answer)==0||zxs_work_reply(answer,0,0)==0)goto done;
	ok=true;
done:
	if(native>=0)close(native);
	if(answer>=0)close(answer);
	if(proc>=0)close(proc);
	close(member[0]);close(member[1]);return ok;
}

static bool one_namespace_root_packet(void)
{
	uint8_t panel[74]={0},zero[16]={0},pool[16]={8},name[ZXS_DIR_NAME+1]={0},role=0;
	struct zxs_smb_tree_cut slot={0};int pair[2],pid=-1,client=-1,storage=-1;
	bool ok=false;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,pair)!=0)return false;
	pid=(int)syscall(SYS_pidfd_open,getpid(),0);if(pid<0)goto done;
	client=zxs_smb_tree_ask(pair[0],zero);if(client<0)goto done;
	storage=zxs_smb_tree_take(pair[1],pid,name);
	if(storage<0 || memcmp(name,zero,16)!=0)goto done;
	memcpy(panel+57,pool,16);
	if(zxs_smb_tree_reply(storage,panel)!=0 || zxs_smb_tree_read(client,zero,&slot)!=0 ||
	   slot.mode!=0 || memcmp(slot.pool,pool,16)!=0)goto done;
	if(zxs_smb_tree_reply(storage,panel)!=0 || zxs_ns_folder_read(client,pool,&slot)==0)goto done;
	close(storage);storage=-1;close(client);client=-1;
	client=zxs_ns_folder_query(pair[0],(const uint8_t *)"Docs",4,1);if(client<0)goto done;
	storage=zxs_dir_take(pair[1],pid,&role,name);
	ok=storage>=0 && role==ZXS_NS_WRITE && name[0]==4 && memcmp(name+1,"Docs",4)==0;
done:
	if(storage>=0)close(storage);
	if(client>=0)close(client);
	if(pid>=0)close(pid);
	close(pair[0]);close(pair[1]);return ok;
}

static bool exact_birth_packet(void)
{
	uint8_t cursor[ZXS_DIR_NAME+1]={0},raw[ZXS_MEMBER_BYTES]={0},other[16]={99},role=0;
	int c[2],pid=-1,native=-1,store=-1;bool ok=false;
	struct zxs_smb_tree_cut f={0};struct zxs_member_info m={0};
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,c)!=0)return false;
	pid=(int)syscall(SYS_pidfd_open,getpid(),0);if(pid<0)goto out;
	native=zxs_birth_query(c[0],(const uint8_t *)"file",4,3);if(native<0)goto out;
	store=zxs_dir_take(c[1],pid,&role,cursor);
	if(store<0 || role!=ZXS_FILE_BIRTH+3 || cursor[0]!=4 || memcmp(cursor+1,"file",4)!=0)goto out;
	raw[0]=f.pool[0]=5;raw[16]=f.volume[0]=6;raw[32]=7;raw[48]=raw[64]=0x32;
	if(zxs_birth_reply(store,raw,ZXS_BIRTH_ACCEPT)!=0 || zxs_birth_result(native,&f,&m)!=ZXS_BIRTH_ACCEPT ||
	   m.file[0]!=0x32 || m.size!=0 ||
	   zxs_birth_ack(native,other)!=0 || zxs_birth_take_ack(store,m.file)==0 ||
	   zxs_birth_ack(native,m.file)!=0 || zxs_birth_take_ack(store,m.file)!=0 ||
	   zxs_birth_done(store,other)!=0 || zxs_birth_read_done(native,m.file)==0 ||
	   zxs_birth_done(store,m.file)!=0 || zxs_birth_read_done(native,m.file)!=0)goto out;
	ok=zxs_birth_reply(store,NULL,ZXS_BIRTH_OCCUPIED)==0 &&
		zxs_birth_result(native,&f,&m)==ZXS_BIRTH_OCCUPIED;
out:
	if(store>=0)close(store);
	if(native>=0)close(native);
	if(pid>=0)close(pid);
	close(c[0]);close(c[1]);return ok;
}

static bool boot_plan_packets(void)
{
	int f[2];uint8_t plan[ZXS_PLAN_SLOT]={0};
	const uint8_t test[]={42,17,81};uint8_t digest[32]={10},other[32]={11};
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,f)!=0)return false;
	bool ok=zxs_smb_plan_send(f[0],test,sizeof(test))==0 &&
		zxs_smb_plan_take(f[1],plan)==sizeof(test) && memcmp(plan,test,sizeof(test))==0 &&
		zxs_smb_plan_reply(f[1],digest)==0 && zxs_smb_plan_ack(f[0],other)!=0 &&
		zxs_smb_plan_reply(f[1],digest)==0 && zxs_smb_plan_ack(f[0],digest)==0;
	close(f[0]);close(f[1]);return ok;
}

static bool retained_round_parent_pair(void)
{
	int p[2],c[2],proc=-1,native=-1,store=-1;bool ok=false;
	uint8_t rows[ZXS_DIR_PACKET-16]={0},got[ZXS_DIR_PACKET-16]={0},after[ZXS_DIR_NAME+1]={0},n=0;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return false;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,c)!=0){close(p[0]);close(p[1]);return false;}
	struct zxs_smb_tree_cut parent={.root_fd=p[0],.volume_ordinal=1,.mode=1,.revision=1},
		child={.root_fd=c[0],.volume_ordinal=1,.mode=1,.revision=1};
	parent.pool[0]=child.pool[0]=1;parent.volume[0]=child.volume[0]=2;child.setting[0]=3;
	proc=(int)syscall(SYS_pidfd_open,getpid(),0);if(proc<0)goto end;
	if(zxs_r_round_own_parent(&child,&parent)!=0)goto end;
	zxs_smb_root_release(&parent); /* borrowed former route can't stop child */
	if(zxs_smb_ctl_live(p[1])!=0||!child.owns_parent)goto end;
	native=zxs_dir_query(child.root_fd,1,NULL,0);if(native<0)goto end;
	store=zxs_dir_take(c[1],proc,&n,after);
	rows[0]=1;rows[1]=1;rows[2]='r';rows[1+1+ZXS_DIR_NAME+6]=1;
	ok=store>=0&&n==1&&zxs_smb_ctl_live(p[1])==0&&zxs_dir_reply(store,rows)==0&&
		zxs_dir_result(native,1,got)==0&&memcmp(got,rows,sizeof(got))==0;
end:
	if(store>=0)close(store);if(native>=0)close(native);if(proc>=0)close(proc);
	zxs_smb_root_release(&child);zxs_smb_root_release(&parent);
	ok=ok&&zxs_smb_ctl_live(p[1])!=0&&zxs_smb_ctl_live(c[1])!=0;
	close(p[1]);close(c[1]);return ok;
}


static bool exact_ordinary_parent_and_rows(void)
{
	int p[2],pid=-1,native=-1,peer=-1;
	uint8_t mode=0,role=0,after[ZXS_DIR_NAME+1],decoded[ZXS_OD_PATH_MAX];
	uint8_t original[74]={0},id[16]={8},op[16]={9},wrong[16]={10};
	uint8_t rows[ZXS_DIR_PACKET-16]={0},got[ZXS_DIR_PACKET-16];
	uint16_t n=0;bool ok=false;
	const uint8_t path[]="Reports\\2027";
	struct zxs_smb_tree_cut parent={.root_fd=-1},picked={.root_fd=-1};
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return false;
	parent.root_fd=p[0];
	pid=(int)syscall(SYS_pidfd_open,getpid(),0);if(pid<0)goto end;
	if(!zxs_odir_relative((const char *)path,true) ||
	   zxs_odir_relative("Reports\\\\2027",true) || zxs_odir_relative("Reports\\..\\2027",true) ||
	   zxs_odir_relative("\\Reports",true) || zxs_odir_relative("Reports\\a:ADS",false))goto end;
	/* A positive FILE kind deliberately has neither a directory tuple
	 * nor a child lease. Missing status cannot become this decision. */
	native=zxs_odir_query(p[0],ZXS_OD_SELECT_KIND,path,sizeof(path)-1);if(native<0)goto end;
	peer=zxs_dir_take(p[1],pid,&role,after);
	if(peer<0 || role!=ZXS_OD_PATH || after[0]!=0 ||
	   zxs_odir_take(peer,&mode,&n,decoded)!=0 || mode!=ZXS_OD_SELECT_KIND ||
	   n!=sizeof(path)-1 || memcmp(path,decoded,n)!=0)goto end;
	parent.volume[0]=3;
	if(zxs_odir_reply(peer,6,NULL,NULL,NULL)!=0 ||
	   zxs_odir_read(native,&parent,&picked)!=6 ||
	   zxs_odir_reply(peer,ZXS_OD_FILE_KIND,original,id,op)==0 ||
	   zxs_odir_reply(peer,ZXS_OD_FILE_KIND,NULL,NULL,NULL)!=0 ||
	   zxs_odir_read(native,&parent,&picked)!=ZXS_OD_FILE_KIND || picked.root_fd!=-1)goto end;
	close(native);native=-1;close(peer);peer=-1;
	native=zxs_odir_query(p[0],3,path,sizeof(path)-1);if(native<0)goto end;
	peer=zxs_dir_take(p[1],pid,&role,after);
	if(peer<0 || role!=ZXS_OD_PATH || after[0]!=0 ||
	   zxs_odir_take(peer,&mode,&n,decoded)!=0 || mode!=3 || n!=sizeof(path)-1 ||
	   memcmp(path,decoded,n)!=0)goto end;
	original[0]=parent.share[0]=2;original[16]=parent.volume[0]=3;
	original[32]=parent.setting[0]=4;original[55]=parent.revision=1;original[56]=parent.mode=2;
	original[57]=parent.pool[0]=5;original[73]=parent.volume_ordinal=2;
	original[32]=6;
	if(zxs_odir_reply(peer,1,original,id,op)!=0 ||
	   zxs_odir_read(native,&parent,&picked)>=0)goto end;
	original[32]=4;
	if(zxs_odir_reply(peer,1,original,id,op)!=0 ||
	   zxs_odir_read(native,&parent,&picked)!=1 ||
	   memcmp(id,picked.directory,16)!=0 || memcmp(op,picked.directory_op,16)!=0)goto end;
	picked.root_fd=native;native=-1;
	if(zxs_odir_own_parent(&picked,&parent)!=0 || parent.root_fd!=-1 ||
	   zxs_smb_ctl_live(p[1])!=0 ||
	   zxs_odir_ack(picked.root_fd,id,wrong)!=0 || zxs_odir_take_ack(peer,id,op)==0 ||
	   zxs_odir_ack(picked.root_fd,id,op)!=0 || zxs_odir_take_ack(peer,id,op)!=0 ||
	   zxs_odir_done(peer,id,op)!=0 || zxs_odir_read_done(picked.root_fd,id,op)!=0)goto end;
	rows[0]=1;rows[1]=1;rows[2]='d';
	memset(rows+1+1+ZXS_DIR_NAME,0xff,8);rows[1+1+ZXS_DIR_NAME+8]=0x80;
	if(zxs_odir_rows(peer,rows)!=0 || zxs_odir_result(picked.root_fd,1,got)!=0 ||
	   memcmp(rows,got,sizeof(got))!=0)goto end;
	if(zxs_dir_reply(peer,rows)!=0 || zxs_dir_result(picked.root_fd,1,got)==0)goto end;
	rows[1+1+ZXS_DIR_NAME+9]=1;
	if(zxs_odir_rows(peer,rows)!=0 || zxs_odir_result(picked.root_fd,1,got)==0)goto end;
	ok=true;
end:
	zxs_smb_root_release(&picked);zxs_smb_root_release(&parent);
	ok=ok&&zxs_smb_ctl_live(p[1])!=0&&zxs_smb_ctl_live(peer)!=0;
	if(peer>=0)close(peer);if(native>=0)close(native);if(pid>=0)close(pid);close(p[1]);return ok;
}

static bool exact_root_capacity_envelope(void)
{
	int p[2],ref=-1,peer=-1,pid=-1;
	uint8_t scope=0,name[ZXS_DIR_NAME+1];
	struct zxs_ns_space got={0};
	bool ok=false;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return false;
	pid=(int)syscall(SYS_pidfd_open,getpid(),0);if(pid<0)goto end;
	ref=zxs_ns_capacity_query(p[0],false);if(ref<0)goto end;
	peer=zxs_dir_take(p[1],pid,&scope,name);
	if(peer<0 || scope!=ZXS_NS_CAPACITY_RO || name[0]!=0 ||
	   zxs_ns_capacity_reply(peer,ZXS_NS_CAP_NO_MEDIA,4096,0,0)==0 ||
	   zxs_ns_capacity_reply(peer,ZXS_NS_CAP_NO_MEDIA,0,0,0)!=0 ||
	   zxs_ns_capacity_read(ref,&got)!=0 || got.status!=ZXS_NS_CAP_NO_MEDIA || got.total!=0)goto end;
	close(ref);ref=-1;close(peer);peer=-1;
	ref=zxs_ns_capacity_query(p[0],true);if(ref<0)goto end;
	peer=zxs_dir_take(p[1],pid,&scope,name);
	if(peer<0 || scope!=ZXS_NS_CAPACITY_RW || name[0]!=0 ||
	   zxs_ns_capacity_reply(peer,ZXS_NS_CAP_READY,16384,8192,4096)==0 ||
	   zxs_ns_capacity_reply(peer,ZXS_NS_CAP_READY,16384,4096,8192)!=0 ||
	   zxs_ns_capacity_read(ref,&got)!=0 || got.status!=ZXS_NS_CAP_READY ||
	   got.total!=16384 || got.available!=4096 || got.free!=8192)goto end;
	uint8_t wrong[48]={0};memcpy(wrong,"ZXS-NS/CAPRES1",14);wrong[17]=1;
	if(send(peer,wrong,sizeof(wrong),0)!=sizeof(wrong) || zxs_ns_capacity_read(ref,&got)==0)goto end;
	ok=true;
end:
	if(ref>=0)close(ref);if(peer>=0)close(peer);if(pid>=0)close(pid);
	close(p[0]);close(p[1]);return ok;
}

static bool exact_transfer_envelope(void)
{
	int p[2],proc=-1,native=-1,store=-1;bool ok=false;
	uint8_t folder[ZXS_DIR_NAME+1],name[ZXS_FILE_NAME+1],kind=0,status=42,file[16]={3},op[16]={9},other[16]={4},mark[16],parents[ZXS_OD_PATH_MAX];
	uint16_t width=0;
	if(socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,p)!=0)return false;
	proc=(int)syscall(SYS_pidfd_open,getpid(),0);if(proc<0)goto end;
	if(zxs_xfer_query(p[0],ZXS_XFER_DELETE,(const uint8_t *)"F",1,NULL,0,NULL)>=0)goto end;
	native=zxs_xfer_query(p[0],ZXS_XFER_RESTORE,(const uint8_t *)"F",1,(const uint8_t *)"Report",6,NULL);
	if(native<0||zxs_member_xfer_pending(p[1])!=1)goto end;
	store=zxs_xfer_take(p[1],proc,&kind,folder,name,mark,&width,parents);
	if(width!=0||store<0||kind!=ZXS_XFER_RESTORE||folder[0]!=1||folder[1]!='F'||name[0]!=6||memcmp(name+1,"Report",6)!=0)goto end;
	if(zxs_xfer_reply(store,0,file,op)!=0||zxs_xfer_read(native,other,&status,NULL)==0)goto end;
	if(zxs_xfer_reply(store,0,file,op)!=0||zxs_xfer_read(native,file,&status,mark)!=0||status!=0)goto end;
	if(zxs_xfer_reply(store,ZXS_XFER_UNKNOWN,NULL,NULL)!=0||zxs_xfer_read(native,file,&status,NULL)!=0||
	   status!=ZXS_XFER_UNKNOWN)goto end;
	close(store);store=-1;close(native);native=-1;
	if(zxs_xfer_query(p[0],ZXS_XFER_DELETE,NULL,0,NULL,0,NULL)>=0)goto end;
	native=zxs_xfer_query(p[0],ZXS_XFER_DELETE,NULL,0,NULL,0,op);
	if(native<0||zxs_member_xfer_pending(p[1])!=1)goto end;
	store=zxs_xfer_take(p[1],proc,&kind,folder,name,mark,&width,parents);
	if(width!=0 || store<0 || kind!=ZXS_XFER_DELETE || folder[0]!=0 || name[0]!=0 || memcmp(mark,op,16)!=0)goto end;
	close(store);store=-1;close(native);native=-1;
	const uint8_t nested[]="Reports\\2027\\Report";
	native=zxs_xfer_query(p[0],ZXS_XFER_RESTORE,(const uint8_t *)"F",1,nested,sizeof(nested)-1,NULL);
	if(native<0||zxs_member_xfer_pending(p[1])!=1)goto end;
	store=zxs_xfer_take(p[1],proc,&kind,folder,name,mark,&width,parents);
	ok=store>=0&&kind==ZXS_XFER_RESTORE&&width==12&&memcmp(parents,"Reports\\2027",12)==0&&
	   name[0]==6&&memcmp(name+1,"Report",6)==0;
end:
	if(store>=0)close(store);if(native>=0)close(native);if(proc>=0)close(proc);
	close(p[0]);close(p[1]);return ok;
}

int main(void)
{
	if(!exact_ordinary_parent_and_rows() || !exact_root_capacity_envelope() || !retained_round_parent_pair() || !exact_transfer_envelope() || !exact_birth_packet() || !one_namespace_root_packet() || !boot_plan_packets() || !directory_pairs() || !held_member_pair() || !staged_writer_pair())return 1;
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
