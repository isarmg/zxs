/* SPDX-License-Identifier: GPL-3.0-or-later
 * Private to this one Samba build. Zero credential material crosses
 * process boundaries or leaves the native SAMU implementation. */
#ifndef ZXS_NATIVE_TDB_H
#define ZXS_NATIVE_TDB_H
bool zxs_native_tdb_audit(const char *fixed_location);
bool zxs_samu_epoch_v4(uint8_t *record,uint32_t length);
NTSTATUS zxs_native_tdb_install(struct pdb_methods **out,const char *location);
#endif
