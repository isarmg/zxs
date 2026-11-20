/* SPDX-License-Identifier: Apache-2.0
 * Startup on the selected private control before it receives worker roles. */
static const uint8_t plan_tag[16]="ZXS-SMB/PLAN-01";
static const uint8_t plan_ok[16]="ZXS-SMB/PLAN-OK";

int zxs_smb_plan_send(int fd,const uint8_t *plan,size_t n)
{
	uint8_t b[ZXS_PLAN_OFFER]={0};
	if(plan==NULL || n==0 || n>ZXS_PLAN_SLOT)return bad();
	memcpy(b,plan_tag,16);b[16]=n>>8;b[17]=n;
	memcpy(b+18,plan,n);
	return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,false);
}
int zxs_smb_plan_take(int fd,uint8_t plan[ZXS_PLAN_SLOT])
{
	uint8_t b[ZXS_PLAN_OFFER];
	if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,false)!=0 ||
	   memcmp(b,plan_tag,16)!=0)return bad();
	size_t n=((size_t)b[16]<<8)|b[17];
	if(n==0 || n>ZXS_PLAN_SLOT)return bad();
	for(size_t i=n;i<ZXS_PLAN_SLOT;i++)if(b[18+i]!=0)return bad();
	memcpy(plan,b+18,n);return n;
}
int zxs_smb_plan_reply(int fd,const uint8_t hash[32])
{
	uint8_t b[ZXS_PLAN_ACK];
	memcpy(b,plan_ok,16);memcpy(b+16,hash,32);
	return transfer(fd,b,NULL,sizeof(b),-1,NULL,ZXS_NO_FD,false);
}
int zxs_smb_plan_ack(int fd,const uint8_t hash[32])
{
	uint8_t b[ZXS_PLAN_ACK];
	if(transfer(fd,NULL,b,sizeof(b),-1,NULL,ZXS_NO_FD,false)!=0 ||
	   memcmp(b,plan_ok,16)!=0 || memcmp(b+16,hash,32)!=0)return bad();
	return 0;
}
