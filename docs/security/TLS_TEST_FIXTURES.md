# Offline TLS regression fixtures

The files in `test/client/integration/fixtures/tls/` are test-only. They are not used by
product code or installed in an operating-system trust store. The synthetic
certificates and test key must never be used for a production service.

- `root.der`, `intermediate.der`, and `localhost.der` form a synthetic P-256
  certificate chain. `generate.py` created them locally, with fixed validity
  from 2020-01-01 through 2120-01-01. The leaf is valid only for `localhost` and
  `127.0.0.1`.
- `localhost-test-key.pkcs8.der` is the matching **public test private key**. It
  was generated solely for the local test server. The CA/intermediate private
  keys were not saved. This key must never be used outside tests.
- `emsign-ecc-root-ca---g3.der` is the public "emSign ECC Root CA - G3"
  certificate removed in `webpki-roots` 1.0.10. It was decoded from the PEM
  comment in `src/foundation/webpki-roots/src/lib.rs` at repository commit
  `75a37e5921fc4c4318310f96d994c16a6e91f0ab` (`webpki-roots` 1.0.9). Its DER SHA-256
  fingerprint is
  `86a1ecba089c4a8d3bbe2734c612ba341d813e043cf9e8a862cd5c57a36bbe6b`.
  The upstream root dataset is licensed CDLA-Permissive-2.0; see the existing
  upstream license records in this repository.

The tests use the eight official public SPKI files from the vendored
`rustls-webpki` 0.103.16 source directly. No compromised private key was fetched,
generated, or included. The public-key substitutions deliberately invalidate
the original certificate signatures: the regression requires a specific
`CertDistrusted` rejection before issuer/signature validation. Separate positive
and tampered-signature controls exercise real cryptographic verification, and
the local HTTPS tests exercise the actual reqwest → rustls → webpki path.

Upstream references:

- <https://github.com/rustls/webpki/pull/546>
- <https://github.com/rustls/webpki-roots/commit/70760478b9d3ea729b22faa0b5324da38a297c1c>
- <https://rustsec.org/advisories/RUSTSEC-2026-0382.html>

Run the tests without network access or new registry dependencies:

```sh
cargo test --offline -j2 -p storage-client --test tls_trust_regression
```

Regeneration is optional and requires Python's `cryptography` package; the test
run uses the committed DER files and does not require Python or OpenSSL:

```sh
python3 test/client/integration/fixtures/tls/generate.py
```

Regeneration creates fresh synthetic keys but does not modify the public
emSign root fixture or the official advisory SPKIs.
