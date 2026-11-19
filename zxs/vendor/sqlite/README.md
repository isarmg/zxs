# Upstream SQLite source, unmodified

This directory contains only `sqlite3.c` and `sqlite3.h` from [SQLite 3.54.1, 2026-10-16](https://sqlite.org/releaselog/3_54_1.html). The independent variable-sized FILE bodies, checksums and indexes are not SQLite rows. No shell, extension headers, prebuilt library, private VFS, system-SQLite fallback, online fetch during build or source changes are included.

Official [amalgamation archive](https://www.sqlite.org/2026/sqlite-amalgamation-3540100.zip), SHA3-256:
- ZIP (2,990,966 bytes): `92281c86e2957f9b9305890f9ac04204caa7b677cc868dc326d942689df28e30`
- `sqlite3.c` (9,582,521 bytes): `e2b869d3003b33c942b7dd7063604fbc53553e1b66609570df39b9b7b558ee81` (also published by upstream)
- `sqlite3.h` (695,729 bytes): `15ef3f5dbc4a7a980f828fa0d1e473cee5a124e7bb77e9bc70fc0bfa3f9a2af8` (measured from that verified archive)
- `SQLITE_SOURCE_ID=2026-10-16 14:45:20 555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76`.

Upstream places the core in the [public domain](https://sqlite.org/copyright.html). Preserve the original notices. Report its 271,131 C and 14,447 header lines separately from ZXS-authored code. The independent native adapter includes this same header and is statically linked to the same sources; compile-time excluded: dynamic extension loading, WAL and shared cache. OMIT_WAL makes the actual SQLite pager refuse a WAL-format main file even when no sidecar is currently named. This prevents a read-only observer from creating WAL/SHM in a writable SYSTEM directory; checking PRAGMA after a WAL-enabled open would be too late. No modification of the upstream C or header. The compiled library is never transferred or run by this change; actual toolchain, crash and media qualification are separate.
