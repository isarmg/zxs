/* SPDX-License-Identifier: GPL-3.0-or-later
 * The dedicated file service accepts one final wire dialect. The regular
 * Samba parser, selection order and 3.1.1 contexts are not reimplemented. */
#ifndef ZXS_MANAGED_DIALECT_H
#define ZXS_MANAGED_DIALECT_H
#include "libcli/smb/smb_constants.h"
static inline bool zxs_smb311_bounds(enum protocol_types low,enum protocol_types high)
{
	return low==PROTOCOL_SMB3_11 && high==PROTOCOL_SMB3_11;
}
enum protocol_types zxs_smb311_offer(const uint8_t *dialects,int count,uint16_t *selected);
#endif
