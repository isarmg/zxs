/* SPDX-License-Identifier: GPL-3.0-or-later
 * Exact, nonsecret account identity in Samba's original counted SAMU fields.
 * It does not parse passwords, authenticate clients or open a database. */
#ifndef ZXS_NATIVE_PUBLIC_H
#define ZXS_NATIVE_PUBLIC_H
#include "param/zxs_smb_control.h"
#include <stdbool.h>
#include <string.h>
static inline bool zxs_native_public_fields(const struct zxs_smb_generation *slot,const uint8_t instance[16],
                               const char *login,size_t nl,const char *domain,size_t nd,
                               const char *description,size_t ns)
{
    static const char hex[]="0123456789abcdef";
    if (slot==NULL || instance==NULL) return false;
    const uint8_t *ids[]={slot->pool,instance,slot->account,slot->credential_op};
    char marker[154]; size_t pos=5;
    if (slot->generation==0 || slot->generation>INT64_MAX || strnlen(slot->login,sizeof(slot->login))>=sizeof(slot->login) || nl!=strlen(slot->login)+1 || login==NULL || domain==NULL ||
        description==NULL || nd!=sizeof("ZXSLOCAL") || ns!=sizeof(marker) ||
        memcmp(login,slot->login,nl)!=0 || memcmp(domain,"ZXSLOCAL",nd)!=0) return false;
    memcpy(marker,"ZXS1:",5);
    for(size_t j=0;j<4;j++) {
        for(size_t i=0;i<16;i++) {
            marker[pos++]=hex[ids[j][i]>>4];marker[pos++]=hex[ids[j][i]&15];
        }
        marker[pos++]=':';
    }
    for(size_t i=0;i<16;i++) marker[pos++]=hex[(slot->generation>>(60-i*4))&15];
    marker[pos++]='\0';
    return pos==sizeof(marker) && memcmp(description,marker,pos)==0;
}
#endif
