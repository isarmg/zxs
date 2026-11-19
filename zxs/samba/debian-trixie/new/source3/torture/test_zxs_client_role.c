/* SPDX-License-Identifier: GPL-3.0-or-later
 * Source qualification only: two independent loopback TCP connections,
 * an auxiliary fork, the selected fork and an inherited grandchild. No
 * Samba service, setup credential, SQL access or positive authentication. */
#include "includes.h"
#include "param/zxs_client_role.h"
#include <arpa/inet.h>
#include <sys/wait.h>

static bool complete(pid_t pid)
{
	int status;
	if (pid<=0) { return false; }
	while (waitpid(pid,&status,0)<0) { if (errno!=EINTR) { return false; } }
	return WIFEXITED(status) && WEXITSTATUS(status)==0;
}
int main(void)
{
	int l=-1,c1=-1,c2=-1,a1=-1,a2=-1;
	int un[2]; struct sockaddr_in a={.sin_family=AF_INET}; socklen_t n=sizeof(a);
	struct zxs_client_role r={.parent=getpid()};
	struct zxs_client_offer o={0},unused={0};
	pid_t p;
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
