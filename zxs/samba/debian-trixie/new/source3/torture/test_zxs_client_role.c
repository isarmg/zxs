/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source qualification only: two independent loopback TCP connections,
 * an auxiliary fork, the selected fork and an inherited grandchild. No
 * Samba service, setup credential, SQL access or positive authentication. */
#include "includes.h"
#include "param/zxs_runtime.h"
#include "param/zxs_receiver.h"
#include <arpa/inet.h>
#include <sys/wait.h>

static bool complete(pid_t pid)
{
	int status;
	if (pid<=0) { return false; }
	while (waitpid(pid,&status,0)<0) { if (errno!=EINTR) { return false; } }
	return WIFEXITED(status) && WEXITSTATUS(status)==0;
}
/* A rejected child still runs with its listener's inherited process state.
 * Ordinary exit would run this inherited-cleanup sentinel. This fixture
 * does not start a live listener or claim to exercise Samba's pidfile. */
static int inherited_cleanup_fd=-1;
static void inherited_cleanup(void)
{
	const char mark='x';
	if (inherited_cleanup_fd>=0) { (void)write(inherited_cleanup_fd,&mark,1); }
}
static bool rejected_before_reinit(void)
{
	int gate[2],witness[2],status;char mark;pid_t child,got;ssize_t seen;
	if (socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,gate)!=0 || pipe(witness)!=0) {
		return false;
	}
	child=fork();
	if (child==0) {
		close(gate[1]);close(witness[0]);inherited_cleanup_fd=witness[1];
		if (atexit(inherited_cleanup)!=0) { _exit(2); }
		zxs_runtime_wait_gate_or_exit(gate[0]);
		_exit(3); /* The closed gate must not hand a worker to smbd_process. */
	}
	close(gate[0]);close(gate[1]);close(witness[1]);
	if (child<=0) { close(witness[0]);return false; }
	do { got=waitpid(child,&status,0); } while (got<0 && errno==EINTR);
	do { seen=read(witness[0],&mark,1); } while (seen<0 && errno==EINTR);
	close(witness[0]);
	return got==child && WIFEXITED(status) && WEXITSTATUS(status)==EXIT_FAILURE && seen==0;
}
/* The irreversibly restricted branch owns no serving socket, password or
 * user data. Its unfiltered parent can still create another worker. */
static bool no_inherited_receiver(void)
{
	pid_t child=fork();
	if (child==0) {
		int pair[2],copy;
		if (!zxs_seal_receiver() || socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,pair)!=0) {
			_exit(2);
		}
		/* Same-process duplicates remain valid. A new process must never
		 * get a chance to receive from even this nonsecret fixture pair. */
		copy=dup(pair[0]);
		if (copy<0) { _exit(3); }
		if (fork()!=-1 || errno!=EPERM) { _exit(4); }
		errno=0;
		if (syscall(SYS_clone3,NULL,0)!=-1 || errno!=ENOSYS) { _exit(5); }
		errno=0;
		if (syscall(SYS_execve,"/cannot-be-an-installed-program",NULL,NULL)!=-1 ||
		    errno!=EPERM) { _exit(6); }
		const unsigned char b=19;unsigned char got=0;
		if (write(pair[1],&b,1)!=1 || read(copy,&got,1)!=1 || got!=b) { _exit(7); }
		_exit(0);
	}
	if (!complete(child)) { return false; }
	child=fork();if (child==0) { _exit(0); }
	return complete(child);
}
int main(void)
{
	int l=-1,c1=-1,c2=-1,a1=-1,a2=-1;
	int un[2]; struct sockaddr_in a={.sin_family=AF_INET}; socklen_t n=sizeof(a);
	struct zxs_client_role r={.parent=getpid()};
	struct zxs_client_offer o={0},unused={0};
	pid_t p;
	if (!rejected_before_reinit() || !no_inherited_receiver()) { return 1; }
	a.sin_addr.s_addr=htonl(INADDR_LOOPBACK);
	l=socket(AF_INET,SOCK_STREAM|SOCK_CLOEXEC,IPPROTO_TCP);
	c1=socket(AF_INET,SOCK_STREAM|SOCK_CLOEXEC,IPPROTO_TCP);
	c2=socket(AF_INET,SOCK_STREAM|SOCK_CLOEXEC,IPPROTO_TCP);
	if (l<0 || c1<0 || c2<0 || bind(l,(struct sockaddr *)&a,n)!=0 ||
	    listen(l,2)!=0 || getsockname(l,(struct sockaddr *)&a,&n)!=0 ||
	    connect(c1,(struct sockaddr *)&a,n)!=0 || (a1=accept(l,NULL,NULL))<0 ||
	    connect(c2,(struct sockaddr *)&a,n)!=0 || (a2=accept(l,NULL,NULL))<0 ||
	    socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,un)!=0) { return 1; }
	if (zxs_offer_accepted(&r,l,&unused) || zxs_offer_accepted(&r,un[0],&unused)) { return 1; }
	/* The auxiliary role fork did not originate an offer or receive one. */
	p=fork();
	if (p==0) { _exit(!zxs_client_here(&r) && !zxs_offer_accepted(&r,a1,&unused) &&
			    !zxs_claim_accepted(&r,&unused,a1) ? 0 : 1); }
	if (!complete(p) || !zxs_offer_accepted(&r,a1,&o) ||
	    zxs_claim_accepted(&r,&o,a1) || zxs_client_here(&r)) { return 1; }
	p=fork();
	if (p==0) {
		int copy; pid_t g;
		if (zxs_claim_accepted(&r,&o,a2) || !zxs_claim_accepted(&r,&o,a1) ||
		    !zxs_client_here(&r) || zxs_claim_accepted(&r,&o,a1) ||
		    !zxs_client_on(&r,a1) || zxs_client_on(&r,a2) ||
		    zxs_client_on(&r,un[0])) { _exit(1); }
		copy=dup(a1);
		if (copy<0 || !zxs_client_on(&r,copy) || dup2(a2,a1)!=a1 ||
		    zxs_client_on(&r,a1) || !zxs_client_on(&r,copy)) { _exit(1); }
		g=fork();
		if (g==0) { _exit(!zxs_client_here(&r) && !zxs_client_on(&r,copy) &&
				   !zxs_claim_accepted(&r,&o,copy) ? 0 : 1); }
		_exit(complete(g) ? 0 : 1);
	}
	return complete(p) && !zxs_client_on(&r,a1) ? 0 : 1;
}
