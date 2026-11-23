/* Bridging header for the private macOS FSKit extension target (Darwin LP64). */
#ifndef ZXS_WORK_EVIDENCE_H
#define ZXS_WORK_EVIDENCE_H
#include <stdint.h>
#include <stddef.h>
int zxs_work_evidence(int fd, int directory, const char *volume_hex, uint64_t inode,
                      int64_t birth_sec, int64_t birth_nsec);
int zxs_work_spelling(int fd, const char *expected);
int zxs_resident_work_leaf(int parent, const char *name, uint64_t inode, int64_t sec, int64_t nsec);
int zxs_open_work_leaf(int parent, const char *name, int flags, uint64_t inode, int64_t sec, int64_t nsec);
int zxs_new_names(int fd, int *sensitive);
int zxs_marker_name(int fd, unsigned char *bytes, size_t cap, size_t *length);
#endif
