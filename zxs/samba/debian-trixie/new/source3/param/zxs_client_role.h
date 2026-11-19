/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private Debian/Linux native transport provenance. Not a credential,
 * account, host attestation, successful login, or client-supplied object. */
#ifndef ZXS_NATIVE_CLIENT_ROLE_H
#define ZXS_NATIVE_CLIENT_ROLE_H
#include <sys/types.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdbool.h>
#include <unistd.h>
struct zxs_tcp_original { uint64_t net, sock; };
struct zxs_client_offer { pid_t parent; struct zxs_tcp_original tcp; };
struct zxs_client_role { pid_t parent, worker; struct zxs_tcp_original tcp; };
static inline bool zxs_tcp_id(int fd,struct zxs_tcp_original *out)
{
	socklen_t n=sizeof(out->sock);
	if (getsockopt(fd,SOL_SOCKET,SO_COOKIE,&out->sock,&n)!=0 ||
	    n!=sizeof(out->sock) || out->sock==0) { return false; }
	n=sizeof(out->net);
	return getsockopt(fd,SOL_SOCKET,SO_NETNS_COOKIE,&out->net,&n)==0 &&
	       n==sizeof(out->net) && out->net!=0;
}
static inline bool zxs_tcp_same(const struct zxs_tcp_original *a,
				const struct zxs_tcp_original *b)
{
	return a->sock==b->sock && a->net==b->net;
}
static inline bool zxs_offer_accepted(const struct zxs_client_role *r,
				      int fd,struct zxs_client_offer *o)
{
	struct sockaddr_storage peer; socklen_t n=sizeof(peer);
	struct zxs_tcp_original id;
	int proto=0,type=0;
	if (r->parent!=getpid() || r->worker!=0 ||
	    getpeername(fd,(struct sockaddr *)&peer,&n)!=0 ||
	    !((peer.ss_family==AF_INET && n==sizeof(struct sockaddr_in)) ||
	      (peer.ss_family==AF_INET6 && n==sizeof(struct sockaddr_in6)))) {
		return false;
	}
	n=sizeof(type);
	if (getsockopt(fd,SOL_SOCKET,SO_TYPE,&type,&n)!=0 || n!=sizeof(type) ||
	    type!=SOCK_STREAM) { return false; }
	n=sizeof(proto);
	if (getsockopt(fd,SOL_SOCKET,SO_PROTOCOL,&proto,&n)!=0 || n!=sizeof(proto) ||
	    proto!=IPPROTO_TCP || !zxs_tcp_id(fd,&id)) { return false; }
	o->parent=r->parent;o->tcp=id;return true;
}
static inline bool zxs_claim_accepted(struct zxs_client_role *r,
				      const struct zxs_client_offer *o,int fd)
{
	struct zxs_tcp_original id;
	if (r->worker!=0 || r->parent<=0 || r->parent==getpid() ||
	    r->parent!=getppid() || o->parent!=r->parent ||
	    !zxs_tcp_id(fd,&id) || !zxs_tcp_same(&id,&o->tcp)) { return false; }
	r->worker=getpid();r->tcp=id;return true;
}
static inline bool zxs_client_here(const struct zxs_client_role *r)
{
	return r->worker==getpid() && r->parent==getppid();
}
static inline bool zxs_client_on(const struct zxs_client_role *r,int fd)
{
	struct zxs_tcp_original id;
	return zxs_client_here(r) && zxs_tcp_id(fd,&id) && zxs_tcp_same(&r->tcp,&id);
}
#endif
