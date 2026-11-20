/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private supervisor/accepted-socket provenance. No replacement address
 * or prefix is selected from an SMB peer or from system interface names. */
#include <arpa/inet.h>
#include <netinet/in.h>
#include <pthread.h>
#include <sys/prctl.h>
#include <sys/resource.h>

struct zxs_lan_slot{int family;uint8_t bits,ip[16];};
static struct {
	uint8_t raw[ZXS_PLAN_SLOT],sha[32];
	struct zxs_lan_slot slots[16];
	size_t length,count;
} zxs_lan;
static int zxs_report_fd=-1;
static bool zxs_listeners=false;

static int zxs_hex_lower(uint8_t b)
{
	if(b>='0'&&b<='9')return b-'0';
	if(b>='a'&&b<='f')return b-'a'+10;
	return -1;
}
bool zxs_runtime_lan_config(const char *text)
{
	uint8_t wire[ZXS_PLAN_SLOT];
	size_t n,at=97;
	if(text==NULL || !record_intact())return false;
	n=strnlen(text,2*ZXS_PLAN_SLOT+1);
	if(n<2*99 || n>2*ZXS_PLAN_SLOT || (n&1)!=0)return false;
	for(size_t i=0;i<n/2;i++){
		int a=zxs_hex_lower(text[2*i]),b=zxs_hex_lower(text[2*i+1]);
		if(a<0||b<0)return false;
		wire[i]=(a<<4)|b;
	}
	n/=2;
	if(memcmp(wire,"ZXS-SMB-FIRST-1",16)!=0 || memcmp(wire+16,held.binding+16,16)!=0 ||
	   memcmp(wire+48,held.binding+32,32)!=0 || wire[96]==0 || wire[96]>16)return false;
	if(zxs_lan.length!=0)return zxs_lan.length==n && memcmp(zxs_lan.raw,wire,n)==0;
	for(size_t i=0;i<wire[96];i++){
		struct zxs_lan_slot *s=&zxs_lan.slots[i];
		size_t width;uint8_t family;
		if(n-at<2)return false;
		family=wire[at++];s->bits=wire[at++];
		if(family==4){s->family=AF_INET;width=4;}else if(family==6){s->family=AF_INET6;width=16;}
		else{return false;}
		if(s->bits==0 || s->bits>width*8 || width>n-at || !nonzero(wire+at,width))return false;
		memcpy(s->ip,wire+at,width);at+=width;
	}
	if(gnutls_hash_fast(GNUTLS_DIG_SHA256,wire,n,zxs_lan.sha)!=0 || !record_intact())return false;
	memcpy(zxs_lan.raw,wire,n);zxs_lan.count=wire[96];zxs_lan.length=n;
	return true; /* storage still validates the complete raw plan and SQL */
}
static void zxs_report_close_child(void)
{
	if(zxs_report_fd>=0){close(zxs_report_fd);zxs_report_fd=-1;}
}
static bool zxs_process_nodump(void)
{
	return prctl(PR_SET_DUMPABLE,0L,0L,0L,0L)==0 && prctl(PR_GET_DUMPABLE,0L,0L,0L,0L)==0;
}
static bool zxs_privacy_begin(void)
{
	struct rlimit zero={0,0};
	return setrlimit(RLIMIT_CORE,&zero)==0 && zxs_process_nodump();
}
void zxs_runtime_after_ids(void)
{
	/* Credential changes may reset kernel dumpable. This closes the
	 * setting on return; it does not change the host sysctl or claim
	 * the individual kernel set-ID operations are atomic with it. */
	if(!zxs_process_nodump())_exit(EXIT_FAILURE);
}
static bool zxs_listen_report_take(void)
{
	if(zxs_smb_ctl_claim(4)!=0)return false;
	zxs_report_fd=fcntl(4,F_DUPFD_CLOEXEC,5);
	if(zxs_report_fd<0 || close(4)!=0 || pthread_atfork(NULL,NULL,zxs_report_close_child)!=0)return false;
	return true;
}

static int zxs_socket_address(int fd,bool local,uint8_t ip[16],uint16_t *port)
{
	struct sockaddr_storage ss={0};socklen_t n=sizeof(ss);
	int rc=local?getsockname(fd,(struct sockaddr *)&ss,&n):getpeername(fd,(struct sockaddr *)&ss,&n);
	if(rc!=0)return -1;
	if(ss.ss_family==AF_INET && n==sizeof(struct sockaddr_in)){
		const struct sockaddr_in *a=(const struct sockaddr_in *)&ss;
		memcpy(ip,&a->sin_addr,4);*port=ntohs(a->sin_port);return AF_INET;
	}
	if(ss.ss_family==AF_INET6 && n==sizeof(struct sockaddr_in6)){
		const struct sockaddr_in6 *a=(const struct sockaddr_in6 *)&ss;
		if(a->sin6_scope_id!=0 || IN6_IS_ADDR_V4MAPPED(&a->sin6_addr))return -1;
		memcpy(ip,&a->sin6_addr,16);*port=ntohs(a->sin6_port);return AF_INET6;
	}
	return -1;
}
static int zxs_bound_lane(int fd)
{
	uint8_t ip[16]={0};uint16_t port=0;
	int family=zxs_socket_address(fd,true,ip,&port);
	if(family<0||port!=445)return -1;
	for(size_t i=0;i<zxs_lan.count;i++){
		const struct zxs_lan_slot *s=&zxs_lan.slots[i];
		if(family==s->family && memcmp(ip,s->ip,family==AF_INET?4:16)==0)return i;
	}
	return -1;
}
static bool zxs_sock_opt(int fd,int key,int value)
{
	int n=0;socklen_t len=sizeof(n);
	return getsockopt(fd,SOL_SOCKET,key,&n,&len)==0 && len==sizeof(n) && n==value;
}
static bool zxs_peer_in_lane(int fd)
{
	uint8_t peer[16]={0};uint16_t port=0;
	int lane=zxs_bound_lane(fd);
	if(lane<0)return false;
	const struct zxs_lan_slot *s=&zxs_lan.slots[lane];
	if(zxs_socket_address(fd,false,peer,&port)!=s->family || port==0)return false;
	unsigned bytes=s->bits/8,rem=s->bits%8;
	return memcmp(peer,s->ip,bytes)==0 &&
		(rem==0 || ((peer[bytes]^s->ip[bytes]) & (0xffu<<(8-rem)))==0);
}
bool zxs_runtime_listener(int fd,uint32_t *seen)
{
	int lane=zxs_bound_lane(fd);
	if(seen==NULL || lane<0 || !zxs_sock_opt(fd,SO_TYPE,SOCK_STREAM) ||
	   !zxs_sock_opt(fd,SO_PROTOCOL,IPPROTO_TCP) || !zxs_sock_opt(fd,SO_ACCEPTCONN,1) ||
	   (*seen&(1u<<lane))!=0)return false;
	*seen|=1u<<lane;return true;
}
bool zxs_runtime_listen_ready(uint32_t seen)
{
	uint8_t bytes[176]={0};
	if(zxs_listeners || zxs_report_fd<0 || client_role.parent!=getpid() ||
	   zxs_lan.count==0 || seen!=(1u<<zxs_lan.count)-1 ||
	   !zxs_runtime_native_epoch(&audited_slot))return false;
	memcpy(bytes,"ZXS-SMB/LISTEN1",16);memcpy(bytes+16,held.binding,128);
	memcpy(bytes+144,zxs_lan.sha,32);
	int fd=zxs_report_fd;zxs_report_fd=-1;
	bool ok=send(fd,bytes,sizeof(bytes),MSG_DONTWAIT|MSG_NOSIGNAL)==sizeof(bytes) && shutdown(fd,SHUT_WR)==0;
	if(close(fd)!=0)return false;
	zxs_listeners=ok;return ok;
}
