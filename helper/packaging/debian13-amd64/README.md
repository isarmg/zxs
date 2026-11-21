# Offline Helper tool materials for Debian 13 amd64

The only maintained recipe here stages the read-only checker inputs and the
separately approved offline credential providers. It does not compile, package
or install Helper or ZXS, create a service or local configuration, or touch a
live disk. ZXS delivery and acceptance concern the one standalone ZXS ELF. The
disposable Helper embed/extract path is separate and not implemented here.

`build.py --inputs DIR --output NEW_DIR` requires an unprivileged, offline
material workspace. The input directory contains the original trusted local
Debian and Debian Security signed indexes, binary archives and corresponding
source/notice material selected by `lock.json`. Do not point at a live
installation or run the recipe just to check whether the inputs are valid.
Material intake retains the original signature, version, SHA-256, expiry and
bounded extraction checks. Stale/missing/mismatched upstream materials fail;
source history does not authorize overriding their clocks.

The new private output has a filesystem-relative `materials/` tree with the
exact six diagnostics, private tool launcher/loader and approved provider
libraries, plus their notices and checksum inventory. No archive script, unit,
public SMB executable, server config, credential policy, or app binary is
emitted. Tool profiles and expected ELF closure remain exact. The offline
provider is not loaded for regular checker/report operations and is not a
runtime dependency of the independent ZXS application.

This is a source recipe. It does not certify a package, installed host, signed
login, freshness for a past archive timestamp, or an accepted ZXS release.
